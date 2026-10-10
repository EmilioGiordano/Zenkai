use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::presets::{Entry, NativeBinary, Preset};
use crate::settings::{AgentId, AgentServer, EnvValue};

const SHIM_EXTENSIONS: [&str; 4] = ["cmd", "bat", "com", "ps1"];
const INTERPRETERS: [&str; 15] = [
    "cmd",
    "powershell",
    "pwsh",
    "wscript",
    "cscript",
    "mshta",
    "bash",
    "sh",
    "python",
    "pythonw",
    "py",
    "deno",
    "bun",
    "perl",
    "ruby",
];
const NODE_CODE_FLAGS: [&str; 8] = [
    "-e",
    "--eval",
    "-p",
    "--print",
    "-r",
    "--require",
    "--import",
    "--input-type",
];
const NODE_CODE_FLAG_PREFIXES: [&str; 3] = ["--loader", "--experimental-loader", "--inspect"];
const NPX_ASSUME_YES: [&str; 2] = ["-y", "--yes"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchError {
    #[error("the command or an argument contains a line break or a NUL character")]
    ControlCharacter,
    #[error(
        "\"{0}\" is a batch or script shim. Zenkai starts agents with node and the package entry point, never through cmd.exe: use the preset for this agent"
    )]
    Shim(String),
    #[error("\"{0}\" is a shell or script host, which Zenkai does not start as an agent")]
    Interpreter(String),
    #[error("\"{0}\" is not an .exe program")]
    NotExecutable(String),
    #[error("\"{0}\" was not found on PATH")]
    NotFound(String),
    #[error("\"{0}\" is a relative path; give the full path of the program")]
    RelativePath(String),
    #[error("Node.js was not found on PATH. {}", crate::detect::NODE_INSTALL_HINT)]
    NodeMissing,
    #[error("node option \"{0}\" runs code from the command line and is not allowed")]
    NodeCodeFlag(String),
    #[error("npx option \"{0}\" is not allowed; only -y or --yes may precede the package")]
    NpxOption(String),
    #[error("npx needs a package, such as \"-y @scope/name@1.2.3\"")]
    NpxPackageMissing,
    #[error("\"{0}\" is not a package with a version, such as @scope/name@1.2.3")]
    PackageSpec(String),
    #[error("the installed package {package} has no usable program: {reason}")]
    Entry { package: String, reason: String },
    #[error("\"{package}\" has no binary for {platform}")]
    UnsupportedPlatform { package: String, platform: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    pub version: String,
}

impl PackageSpec {
    pub fn parse(text: &str) -> Result<PackageSpec, LaunchError> {
        let invalid = || LaunchError::PackageSpec(text.to_string());
        let (name, version) = text
            .char_indices()
            .skip(1)
            .find(|(_, c)| *c == '@')
            .map(|(at, _)| (&text[..at], &text[at + 1..]))
            .ok_or_else(invalid)?;
        let scoped = name.starts_with('@');
        let segments: Vec<&str> = name.strip_prefix('@').unwrap_or(name).split('/').collect();
        let shape_ok = segments.len() == if scoped { 2 } else { 1 };
        let name_ok = segments.iter().all(|segment| {
            !segment.is_empty()
                && !segment.starts_with('.')
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
        });
        let version_ok = !version.is_empty()
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
        if shape_ok && name_ok && version_ok {
            Ok(PackageSpec {
                name: name.to_string(),
                version: version.to_string(),
            })
        } else {
            Err(invalid())
        }
    }

    pub fn spec(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    fn folder_name(&self) -> String {
        self.spec().replace('/', "_")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchPlan {
    Direct {
        program: PathBuf,
        args: Vec<String>,
    },
    Package {
        node: PathBuf,
        package: PackageSpec,
        folder: PathBuf,
        args: Vec<String>,
    },
    Native {
        node: PathBuf,
        program: PathBuf,
        package: PackageSpec,
        folder: PathBuf,
        args: Vec<String>,
    },
}

impl LaunchPlan {
    pub fn describe(&self) -> String {
        match self {
            LaunchPlan::Direct { program, args } => std::iter::once(program.display().to_string())
                .chain(args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" "),
            LaunchPlan::Package {
                node,
                package,
                folder,
                args,
            } => {
                let mut line = format!(
                    "{} {} (npm package {}, installed under {})",
                    node.display(),
                    package.name,
                    package.spec(),
                    folder.display()
                );
                for arg in args {
                    line.push(' ');
                    line.push_str(arg);
                }
                line
            }
            LaunchPlan::Native {
                program,
                package,
                folder,
                args,
                ..
            } => {
                let mut line = format!(
                    "{} (npm package {}, installed under {})",
                    program.display(),
                    package.spec(),
                    folder.display()
                );
                for arg in args {
                    line.push(' ');
                    line.push_str(arg);
                }
                line
            }
        }
    }
}

pub struct LaunchEnvironment<'a> {
    pub find: &'a dyn Fn(&str) -> Option<PathBuf>,
    pub agents_folder: PathBuf,
}

fn file_name(text: &str) -> &str {
    text.rsplit(['/', '\\']).next().unwrap_or(text)
}

// Windows ignores trailing dots and spaces in a file name, so "x.cmd." still runs a batch file.
fn normalized_name(text: &str) -> String {
    file_name(text)
        .trim_end_matches(['.', ' '])
        .to_ascii_lowercase()
}

fn extension(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, extension)| extension)
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn has_control_character(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '\0' | '\n' | '\r'))
}

fn is_path(text: &str) -> bool {
    text.contains(['/', '\\']) || text.chars().nth(1) == Some(':')
}

fn check_program(command: &str, resolved: &Path) -> Result<(), LaunchError> {
    let name = normalized_name(&resolved.to_string_lossy());
    let shown = || resolved.display().to_string();
    if extension(&name).is_some_and(|ext| SHIM_EXTENSIONS.contains(&ext)) {
        return Err(LaunchError::Shim(shown()));
    }
    let program_stem = stem(&name);
    if INTERPRETERS.contains(&program_stem) || program_stem.starts_with("python") {
        return Err(LaunchError::Interpreter(shown()));
    }
    if cfg!(windows) && extension(&name) != Some("exe") {
        return Err(LaunchError::NotExecutable(command.to_string()));
    }
    Ok(())
}

fn check_node_args(args: &[String]) -> Result<(), LaunchError> {
    for arg in args {
        let flag = arg.split('=').next().unwrap_or(arg);
        // Short flags can be bundled: -pe is --print --eval.
        let bundled = flag.len() > 2
            && flag.starts_with('-')
            && !flag.starts_with("--")
            && flag.chars().skip(1).all(|c| c.is_ascii_alphabetic())
            && flag.chars().skip(1).any(|c| matches!(c, 'e' | 'p' | 'r'));
        let forbidden = bundled
            || NODE_CODE_FLAGS.contains(&flag)
            || NODE_CODE_FLAG_PREFIXES
                .iter()
                .any(|prefix| flag.starts_with(prefix));
        if forbidden {
            return Err(LaunchError::NodeCodeFlag(arg.clone()));
        }
    }
    Ok(())
}

fn plan_package(
    arguments: &[String],
    environment: &LaunchEnvironment,
) -> Result<LaunchPlan, LaunchError> {
    let mut rest = arguments.iter().peekable();
    while let Some(option) = rest.next_if(|arg| arg.starts_with('-')) {
        if !NPX_ASSUME_YES.contains(&option.as_str()) {
            return Err(LaunchError::NpxOption(option.clone()));
        }
    }
    let package = PackageSpec::parse(rest.next().ok_or(LaunchError::NpxPackageMissing)?)?;
    let args: Vec<String> = rest.cloned().collect();
    let node = (environment.find)("node").ok_or(LaunchError::NodeMissing)?;
    check_program("node", &node)?;
    let folder = environment.agents_folder.join(package.folder_name());
    if let Some(preset) = Preset::for_package(&package.spec())
        && let Entry::Native(binaries) = preset.entry
    {
        return plan_native(&package, binaries, args, node, folder);
    }
    Ok(LaunchPlan::Package {
        node,
        package,
        folder,
        args,
    })
}

pub fn platform_name() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn plan_native(
    package: &PackageSpec,
    binaries: &[NativeBinary],
    args: Vec<String>,
    node: PathBuf,
    folder: PathBuf,
) -> Result<LaunchPlan, LaunchError> {
    let relative = binaries
        .iter()
        .find(|binary| binary.os == std::env::consts::OS && binary.arch == std::env::consts::ARCH)
        .map(|binary| binary.path)
        .ok_or_else(|| LaunchError::UnsupportedPlatform {
            package: package.spec(),
            platform: platform_name(),
        })?;
    let program = join_inside(&folder, package, relative)?;
    check_program(&package.spec(), &program)?;
    Ok(LaunchPlan::Native {
        node,
        program,
        package: package.clone(),
        folder,
        args,
    })
}

// The binary a native preset runs, checked after the install: it must have arrived with
// the platform package, so a missing file is reported instead of spawned.
pub fn native_program(
    folder: &Path,
    package: &PackageSpec,
    relative: &str,
) -> Result<PathBuf, LaunchError> {
    let program = join_inside(folder, package, relative)?;
    if program.is_file() {
        Ok(program)
    } else {
        Err(entry_error(
            package,
            format!("{} is missing", program.display()),
        ))
    }
}

fn join_inside(
    folder: &Path,
    package: &PackageSpec,
    relative: &str,
) -> Result<PathBuf, LaunchError> {
    let relative = Path::new(relative);
    if !stays_inside(relative) {
        return Err(entry_error(package, "the binary path leaves the package"));
    }
    let program = folder.join(relative);
    if program.is_absolute() {
        Ok(program)
    } else {
        Err(entry_error(package, "the install folder is not absolute"))
    }
}

fn stays_inside(relative: &Path) -> bool {
    relative
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

pub fn plan(
    server: &AgentServer,
    environment: &LaunchEnvironment,
) -> Result<LaunchPlan, LaunchError> {
    let command = server.command.trim();
    if has_control_character(command) || server.args.iter().any(|arg| has_control_character(arg)) {
        return Err(LaunchError::ControlCharacter);
    }
    let name = normalized_name(command);
    if !is_path(command) && stem(&name) == "npx" {
        return plan_package(&server.args, environment);
    }
    let program = if is_path(command) {
        let path = PathBuf::from(command);
        if !path.is_absolute() {
            return Err(LaunchError::RelativePath(command.to_string()));
        }
        path
    } else {
        (environment.find)(command).ok_or_else(|| LaunchError::NotFound(command.to_string()))?
    };
    check_program(command, &program)?;
    if stem(&normalized_name(&program.to_string_lossy())) == "node" {
        check_node_args(&server.args)?;
    }
    Ok(LaunchPlan::Direct {
        program,
        args: server.args.clone(),
    })
}

pub const INSTALLED_MARKER: &str = ".zenkai-installed";

pub fn npm_cli(node: &Path) -> PathBuf {
    node.parent()
        .unwrap_or(Path::new(""))
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js")
}

fn entry_error(package: &PackageSpec, reason: impl Into<String>) -> LaunchError {
    LaunchError::Entry {
        package: package.spec(),
        reason: reason.into(),
    }
}

// The program npm links for a package, read from its package.json: either a single path or
// a map keyed by command name, where the one named like the package wins.
pub fn package_entry(folder: &Path, package: &PackageSpec) -> Result<PathBuf, LaunchError> {
    let package_folder = folder.join("node_modules").join(&package.name);
    let manifest = std::fs::read_to_string(package_folder.join("package.json"))
        .map_err(|error| entry_error(package, format!("package.json: {error}")))?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest)
        .map_err(|error| entry_error(package, format!("package.json: {error}")))?;
    let short_name = package.name.rsplit('/').next().unwrap_or(&package.name);
    let relative = match manifest.get("bin") {
        Some(serde_json::Value::String(path)) => path.as_str(),
        Some(serde_json::Value::Object(map)) => map
            .get(short_name)
            .or_else(|| {
                if map.len() == 1 {
                    map.values().next()
                } else {
                    None
                }
            })
            .and_then(|value| value.as_str())
            .ok_or_else(|| entry_error(package, "no bin entry named like the package"))?,
        _ => return Err(entry_error(package, "package.json has no bin")),
    };
    let relative = Path::new(relative);
    if !stays_inside(relative) {
        return Err(entry_error(package, "the bin path leaves the package"));
    }
    let entry = package_folder.join(relative);
    if entry.is_file() {
        Ok(entry)
    } else {
        Err(entry_error(
            package,
            format!("{} is missing", entry.display()),
        ))
    }
}

// What a user confirms before the first launch: everything that decides which code runs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LaunchSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl LaunchSpec {
    pub fn of(server: &AgentServer) -> LaunchSpec {
        LaunchSpec {
            command: server.command.clone(),
            args: server.args.clone(),
            env: server
                .env
                .iter()
                .map(|(name, value)| {
                    let shown = match value {
                        EnvValue::Text(text) => text.clone(),
                        EnvValue::Secret { secret } => format!("(secret {secret})"),
                    };
                    (name.clone(), shown)
                })
                .collect(),
        }
    }
}

impl fmt::Display for LaunchSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.command)?;
        for arg in &self.args {
            write!(f, " {arg}")?;
        }
        for (name, value) in &self.env {
            write!(f, "\n{name}={value}")?;
        }
        Ok(())
    }
}

// Launch commands the user approved in this run. They stay in memory on purpose:
// settings.json is meant to be written by agents, and anything that can write it could write
// a file of approvals next to it.
#[derive(Clone, Debug)]
struct Approved {
    spec: LaunchSpec,
    // The executable the approval covers. Approvals made on the Settings page start without one
    // and pin the first program the command resolves to.
    program: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct LaunchApprovals {
    approved: BTreeMap<AgentId, Approved>,
}

fn program_of(plan: &LaunchPlan) -> &Path {
    match plan {
        LaunchPlan::Direct { program, .. } => program,
        LaunchPlan::Package { node, .. } => node,
        LaunchPlan::Native { program, .. } => program,
    }
}

impl LaunchApprovals {
    // A changed command, argument, variable or resolved executable is a new launch.
    pub fn is_approved(&mut self, id: &AgentId, server: &AgentServer, plan: &LaunchPlan) -> bool {
        let spec = LaunchSpec::of(server);
        if crate::presets::PRESETS.iter().any(|preset| {
            matches!(preset.entry, Entry::Node) && LaunchSpec::of(&preset.server()) == spec
        }) {
            return true;
        }
        match self.approved.get_mut(id) {
            Some(approved) if approved.spec == spec => match &approved.program {
                Some(program) => program == program_of(plan),
                None => {
                    approved.program = Some(program_of(plan).to_path_buf());
                    true
                }
            },
            _ => false,
        }
    }

    pub fn approve(&mut self, id: &AgentId, server: &AgentServer, plan: Option<&LaunchPlan>) {
        self.approved.insert(
            id.clone(),
            Approved {
                spec: LaunchSpec::of(server),
                program: plan.map(|plan| program_of(plan).to_path_buf()),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{CLAUDE, CODEX, GEMINI, OPENCODE};
    use crate::settings::SecretName;

    fn server(command: &str, args: &[&str]) -> AgentServer {
        AgentServer {
            name: "Test".to_string(),
            command: command.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            env: BTreeMap::new(),
        }
    }

    fn plan_with(
        server: &AgentServer,
        installed: &[(&str, &str)],
    ) -> Result<LaunchPlan, LaunchError> {
        plan_in(server, installed, PathBuf::from("C:\\agents"))
    }

    fn plan_in(
        server: &AgentServer,
        installed: &[(&str, &str)],
        folder: PathBuf,
    ) -> Result<LaunchPlan, LaunchError> {
        let installed: Vec<(String, PathBuf)> = installed
            .iter()
            .map(|(name, path)| (name.to_string(), PathBuf::from(path)))
            .collect();
        let find = move |name: &str| {
            installed
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, path)| path.clone())
        };
        plan(
            server,
            &LaunchEnvironment {
                find: &find,
                agents_folder: folder,
            },
        )
    }

    fn absolute(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("C:\\{name}"))
        } else {
            PathBuf::from(format!("/{name}"))
        }
    }

    const NODE: (&str, &str) = ("node", "C:\\Program Files\\nodejs\\node.exe");

    #[test]
    fn a_preset_becomes_node_plus_a_managed_package_never_npx() {
        let plan = plan_with(&CLAUDE.server(), &[NODE]).unwrap();
        let LaunchPlan::Package {
            node,
            package,
            folder,
            args,
        } = plan
        else {
            panic!("expected a package plan");
        };
        assert_eq!(node, PathBuf::from(NODE.1));
        assert_eq!(package.name, "@agentclientprotocol/claude-agent-acp");
        assert_eq!(package.version, "0.88.0");
        assert!(folder.ends_with("@agentclientprotocol_claude-agent-acp@0.88.0"));
        assert!(args.is_empty());
    }

    #[test]
    fn extra_preset_arguments_follow_the_package() {
        let plan = plan_with(&GEMINI.server(), &[NODE]).unwrap();
        let LaunchPlan::Package { args, .. } = plan else {
            panic!("expected a package plan");
        };
        assert_eq!(args, ["--acp"]);
    }

    #[test]
    fn node_presets_still_plan_node_with_the_package_entry() {
        let plan = plan_with(&CODEX.server(), &[NODE]).unwrap();
        let LaunchPlan::Package {
            node,
            package,
            args,
            ..
        } = plan
        else {
            panic!("expected a package plan");
        };
        assert_eq!(node, PathBuf::from(NODE.1));
        assert_eq!(package.spec(), "@agentclientprotocol/codex-acp@2.1.1");
        assert!(args.is_empty());
    }

    #[test]
    fn a_native_preset_plans_its_platform_binary() {
        let agents = absolute("agents");
        let planned = plan_in(&OPENCODE.server(), &[NODE], agents.clone());
        match OPENCODE.native_path() {
            Some(native) => {
                let plan = planned.unwrap();
                let LaunchPlan::Native {
                    node,
                    program,
                    package,
                    folder,
                    args,
                    ..
                } = plan
                else {
                    panic!("expected a native plan");
                };
                assert_eq!(node, PathBuf::from(NODE.1));
                assert_eq!(folder, agents.join("opencode-ai@1.18.32"));
                assert_eq!(program, folder.join(native));
                assert_eq!(package.spec(), "opencode-ai@1.18.32");
                assert_eq!(args, ["acp"]);
            }
            None => assert!(matches!(
                planned,
                Err(LaunchError::UnsupportedPlatform { .. })
            )),
        }
    }

    fn current_platform_binaries(path: &'static str) -> [NativeBinary; 1] {
        [NativeBinary {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            path,
        }]
    }

    #[test]
    fn a_native_path_that_leaves_the_folder_is_refused() {
        let binaries = current_platform_binaries("../../evil.exe");
        let package = PackageSpec::parse("opencode-ai@1.18.32").unwrap();
        let folder = tempfile::tempdir().unwrap();
        let planned = plan_native(
            &package,
            &binaries,
            vec!["acp".to_string()],
            PathBuf::from(NODE.1),
            folder.path().to_path_buf(),
        );
        assert!(matches!(planned, Err(LaunchError::Entry { .. })));
        assert!(matches!(
            native_program(folder.path(), &package, "../../evil.exe"),
            Err(LaunchError::Entry { .. })
        ));
    }

    #[test]
    fn the_native_binary_must_exist_after_the_install() {
        let folder = tempfile::tempdir().unwrap();
        let package = PackageSpec::parse("opencode-ai@1.18.32").unwrap();
        let relative = "node_modules/opencode-windows-x64/bin/opencode.exe";
        assert!(matches!(
            native_program(folder.path(), &package, relative),
            Err(LaunchError::Entry { .. })
        ));
        let program = folder.path().join(relative);
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, "").unwrap();
        assert_eq!(
            native_program(folder.path(), &package, relative).unwrap(),
            program
        );
    }

    #[test]
    fn a_platform_with_no_binary_is_a_typed_error() {
        let binaries = [NativeBinary {
            os: "plan9",
            arch: "mips",
            path: "bin/agent",
        }];
        let package = PackageSpec::parse("opencode-ai@1.18.32").unwrap();
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(
            plan_native(
                &package,
                &binaries,
                vec![],
                PathBuf::from(NODE.1),
                folder.path().to_path_buf(),
            ),
            Err(LaunchError::UnsupportedPlatform {
                package: "opencode-ai@1.18.32".to_string(),
                platform: platform_name(),
            })
        );
    }

    fn native_plan(program: &str) -> LaunchPlan {
        LaunchPlan::Native {
            node: PathBuf::from(NODE.1),
            program: PathBuf::from(program),
            package: PackageSpec::parse("opencode-ai@1.18.32").unwrap(),
            folder: PathBuf::from("C:\\agents"),
            args: vec!["acp".to_string()],
        }
    }

    #[test]
    fn a_native_preset_pins_its_executable_like_any_other_program() {
        let mut approvals = LaunchApprovals::default();
        let id = AgentId::new("opencode");
        let server = OPENCODE.server();
        let first = native_plan("C:\\agents\\opencode.exe");
        assert!(!approvals.is_approved(&id, &server, &first));
        approvals.approve(&id, &server, Some(&first));
        assert!(approvals.is_approved(&id, &server, &first));
        assert!(!approvals.is_approved(&id, &server, &native_plan("C:\\planted\\opencode.exe")));
    }

    #[test]
    fn npx_without_node_reports_node_missing() {
        assert_eq!(
            plan_with(&CLAUDE.server(), &[]),
            Err(LaunchError::NodeMissing)
        );
    }

    #[test]
    fn npx_options_other_than_yes_are_refused() {
        for option in ["-c", "--call", "-p", "--package=x", "--shell"] {
            let result = plan_with(&server("npx", &[option, "pkg@1.0.0"]), &[NODE]);
            assert_eq!(result, Err(LaunchError::NpxOption(option.to_string())));
        }
        assert_eq!(
            plan_with(&server("npx", &["-y"]), &[NODE]),
            Err(LaunchError::NpxPackageMissing)
        );
    }

    #[test]
    fn packages_must_be_a_name_with_a_version() {
        for bad in [
            "pkg",
            "git+https://example.com/x.git",
            "file:../x",
            "../x@1",
            "@scope@1",
            "pkg@",
            "UPPER@1.0.0",
            "a/b@1.0.0",
            "@a/b/c@1.0.0",
            "pkg@1 && calc",
        ] {
            assert!(PackageSpec::parse(bad).is_err(), "{bad}");
        }
        let scoped = PackageSpec::parse("@google/gemini-cli@0.63.0").unwrap();
        assert_eq!(scoped.name, "@google/gemini-cli");
        assert_eq!(PackageSpec::parse("tool@latest").unwrap().version, "latest");
    }

    #[test]
    fn batch_and_script_shims_are_never_started() {
        for command in [
            "C:\\tools\\x.cmd",
            "C:\\tools\\x.BAT",
            "C:\\tools\\x.cmd.",
            "C:\\tools\\x.cmd ",
            "C:\\tools\\x.ps1",
        ] {
            let result = plan_with(&server(command, &[]), &[]);
            assert!(
                matches!(result, Err(LaunchError::Shim(_))),
                "{command}: {result:?}"
            );
        }
    }

    #[test]
    fn a_bare_name_that_resolves_to_a_shim_is_refused() {
        let result = plan_with(
            &server("claude", &[]),
            &[("claude", "C:\\Users\\me\\AppData\\Roaming\\npm\\claude.cmd")],
        );
        assert!(matches!(result, Err(LaunchError::Shim(_))));
    }

    #[test]
    fn shells_and_script_hosts_are_not_agents() {
        for command in [
            "C:\\Windows\\System32\\cmd.exe",
            "C:\\x\\PowerShell.exe",
            "C:\\x\\pwsh.exe",
            "C:\\x\\wscript.exe",
        ] {
            let result = plan_with(&server(command, &["/c", "calc"]), &[]);
            assert!(
                matches!(result, Err(LaunchError::Interpreter(_))),
                "{command}"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn only_exe_programs_run_directly() {
        let result = plan_with(&server("C:\\tools\\agent.js", &[]), &[]);
        assert!(matches!(result, Err(LaunchError::NotExecutable(_))));
        let ok = plan_with(&server("C:\\tools\\agent.exe", &["acp"]), &[]).unwrap();
        assert_eq!(
            ok,
            LaunchPlan::Direct {
                program: PathBuf::from("C:\\tools\\agent.exe"),
                args: vec!["acp".to_string()]
            }
        );
    }

    #[test]
    fn relative_commands_and_unknown_names_are_refused() {
        assert!(matches!(
            plan_with(&server(".\\agent.exe", &[]), &[]),
            Err(LaunchError::RelativePath(_))
        ));
        assert!(matches!(
            plan_with(&server("..\\agent.exe", &[]), &[]),
            Err(LaunchError::RelativePath(_))
        ));
        assert_eq!(
            plan_with(&server("agent", &[]), &[]),
            Err(LaunchError::NotFound("agent".to_string()))
        );
    }

    #[test]
    fn line_breaks_and_nul_in_the_command_or_arguments_are_refused() {
        for (command, arg) in [
            ("C:\\a.exe\nx", "--ok"),
            ("C:\\a.exe", "--x\r\ny"),
            ("C:\\a.exe", "a\0b"),
        ] {
            assert_eq!(
                plan_with(&server(command, &[arg]), &[]),
                Err(LaunchError::ControlCharacter)
            );
        }
    }

    #[test]
    fn node_may_not_run_code_from_the_command_line() {
        for flag in [
            "-e",
            "--eval=process.exit()",
            "-p",
            "--print",
            "-r",
            "--require",
            "--import",
            "--loader=x",
            "--experimental-loader",
            "--inspect-brk",
            "--input-type=module",
        ] {
            let result = plan_with(&server("node", &[flag, "x"]), &[NODE]);
            assert!(
                matches!(result, Err(LaunchError::NodeCodeFlag(_))),
                "{flag}"
            );
        }
        let ok = plan_with(&server("node", &["C:\\agent\\index.js", "--acp"]), &[NODE]);
        assert!(ok.is_ok());
    }

    #[test]
    fn the_description_shows_the_resolved_node_and_package() {
        let plan = plan_with(&CLAUDE.server(), &[NODE]).unwrap();
        let text = plan.describe();
        assert!(text.contains("node.exe"));
        assert!(text.contains("@agentclientprotocol/claude-agent-acp@0.88.0"));
    }

    fn installed_package(manifest: &str, files: &[&str]) -> (tempfile::TempDir, PackageSpec) {
        let package = PackageSpec::parse("@acme/tool-acp@1.0.0").unwrap();
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("node_modules/@acme/tool-acp");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("package.json"), manifest).unwrap();
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        (folder, package)
    }

    #[test]
    fn the_entry_comes_from_a_string_or_a_named_bin() {
        let (folder, package) = installed_package(r#"{"bin": "dist/run.js"}"#, &["dist/run.js"]);
        let entry = package_entry(folder.path(), &package).unwrap();
        assert!(entry.ends_with("dist/run.js"));
        let (folder, package) = installed_package(
            r#"{"bin": {"other": "a.js", "tool-acp": "b.js"}}"#,
            &["a.js", "b.js"],
        );
        assert!(
            package_entry(folder.path(), &package)
                .unwrap()
                .ends_with("b.js")
        );
        let (folder, package) = installed_package(r#"{"bin": {"only": "a.js"}}"#, &["a.js"]);
        assert!(package_entry(folder.path(), &package).is_ok());
    }

    #[test]
    fn an_entry_that_leaves_the_package_or_is_missing_is_refused() {
        let (folder, package) = installed_package(r#"{"bin": "../../evil.js"}"#, &[]);
        assert!(matches!(
            package_entry(folder.path(), &package),
            Err(LaunchError::Entry { .. })
        ));
        let (folder, package) = installed_package(r#"{"bin": "C:\\evil.js"}"#, &[]);
        assert!(package_entry(folder.path(), &package).is_err());
        let (folder, package) = installed_package(r#"{"bin": "gone.js"}"#, &[]);
        assert!(package_entry(folder.path(), &package).is_err());
        let (folder, package) = installed_package(r#"{"bin": {"a": "a.js", "b": "b.js"}}"#, &[]);
        assert!(package_entry(folder.path(), &package).is_err());
        let (folder, package) = installed_package("{}", &[]);
        assert!(package_entry(folder.path(), &package).is_err());
    }

    fn direct(program: &str) -> LaunchPlan {
        LaunchPlan::Direct {
            program: PathBuf::from(program),
            args: Vec::new(),
        }
    }

    #[test]
    fn a_different_executable_behind_the_same_command_needs_a_new_approval() {
        let mut approvals = LaunchApprovals::default();
        let id = AgentId::new("mine");
        let typed = server("agent", &[]);
        approvals.approve(&id, &typed, Some(&direct("C:/tools/agent.exe")));
        assert!(approvals.is_approved(&id, &typed, &direct("C:/tools/agent.exe")));
        assert!(!approvals.is_approved(&id, &typed, &direct("C:/planted/agent.exe")));
    }

    #[test]
    fn a_settings_page_approval_pins_the_first_executable_it_resolves_to() {
        let mut approvals = LaunchApprovals::default();
        let id = AgentId::new("mine");
        let typed = server("agent", &[]);
        approvals.approve(&id, &typed, None);
        assert!(approvals.is_approved(&id, &typed, &direct("C:/tools/agent.exe")));
        assert!(!approvals.is_approved(&id, &typed, &direct("C:/planted/agent.exe")));
    }

    #[test]
    fn bundled_short_flags_and_more_interpreters_are_refused() {
        for flag in ["-pe", "-ep", "-rp"] {
            let result = plan_with(&server("node", &[flag, "x"]), &[NODE]);
            assert!(
                matches!(result, Err(LaunchError::NodeCodeFlag(_))),
                "{flag}"
            );
        }
        for command in [
            "C:/py/python.exe",
            "C:/py/python3.12.exe",
            "C:/x/deno.exe",
            "C:/x/bun.exe",
        ] {
            let result = plan_with(&server(command, &[]), &[]);
            assert!(
                matches!(result, Err(LaunchError::Interpreter(_))),
                "{command}"
            );
        }
    }

    #[test]
    fn presets_are_trusted_and_other_commands_need_an_approval() {
        let mut approvals = LaunchApprovals::default();
        let id = AgentId::new("claude");
        let plan = direct("C:/bin/node.exe");
        assert!(approvals.is_approved(&id, &CLAUDE.server(), &plan));
        let mut edited = CLAUDE.server();
        edited.args.push("--extra".to_string());
        assert!(!approvals.is_approved(&id, &edited, &plan));
        approvals.approve(&id, &edited, Some(&plan));
        assert!(approvals.is_approved(&id, &edited, &plan));
        assert!(!approvals.is_approved(&AgentId::new("other"), &edited, &plan));
    }

    #[test]
    fn an_environment_change_needs_a_new_approval() {
        let mut approvals = LaunchApprovals::default();
        let id = AgentId::new("mine");
        let plain = server("C://a.exe", &[]);
        let plan = direct("C:/a.exe");
        approvals.approve(&id, &plain, Some(&plan));
        let mut with_options = plain.clone();
        with_options.env.insert(
            "NODE_OPTIONS".to_string(),
            EnvValue::Text("--require x.js".to_string()),
        );
        assert!(!approvals.is_approved(&id, &with_options, &plan));
        let mut with_secret = plain;
        with_secret.env.insert(
            "KEY".to_string(),
            EnvValue::Secret {
                secret: SecretName::try_from("zenkai/key".to_string()).unwrap(),
            },
        );
        assert!(
            LaunchSpec::of(&with_secret)
                .to_string()
                .contains("(secret zenkai/key)")
        );
    }
}

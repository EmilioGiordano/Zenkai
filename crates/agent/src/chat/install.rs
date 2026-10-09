use std::path::Path;

use async_process::{Command as ProcessCommand, Stdio};

use crate::chat::launch::{self, INSTALLED_MARKER, PackageSpec};
use crate::chat::process::ProcessTree;
use crate::chat::session::{SessionError, hide_window};
use crate::presets::Preset;

const REGISTRY: &str = "https://registry.npmjs.org/";
const EMPTY_USER_NPMRC: &str = "empty-user.npmrc";
const EMPTY_GLOBAL_NPMRC: &str = "empty-global.npmrc";

fn failed(error: impl std::fmt::Display) -> SessionError {
    SessionError::Install(error.to_string())
}

// Installs a package under Zenkai's own folder. npm runs inside that folder with empty user and
// global config files and a pinned registry, so a .npmrc elsewhere cannot redirect it. A preset
// is installed from its committed lockfile with `npm ci`, which verifies every tarball hash; the
// completion marker is written only after npm succeeds, so a half install is redone.
pub(super) async fn install(
    node: &Path,
    package: &PackageSpec,
    folder: &Path,
    tree: &ProcessTree,
) -> Result<(), SessionError> {
    std::fs::create_dir_all(folder).map_err(failed)?;
    let marker = folder.join(INSTALLED_MARKER);
    if marker.exists() {
        std::fs::remove_file(&marker).map_err(failed)?;
    }
    let (user_config, global_config) = (
        folder.join(EMPTY_USER_NPMRC),
        folder.join(EMPTY_GLOBAL_NPMRC),
    );
    std::fs::write(&user_config, "").map_err(failed)?;
    std::fs::write(&global_config, "").map_err(failed)?;
    let mut command = ProcessCommand::new(node);
    command.arg(launch::npm_cli(node));
    match Preset::for_package(&package.spec()) {
        Some(preset) => {
            std::fs::write(folder.join("package.json"), preset.lock.0).map_err(failed)?;
            std::fs::write(folder.join("package-lock.json"), preset.lock.1).map_err(failed)?;
            command.arg("ci");
        }
        None => {
            command.args(["install", "--prefix"]).arg(folder);
            command.arg(package.spec());
        }
    }
    command
        .args([
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
        ])
        .arg(format!("--registry={REGISTRY}"))
        .arg("--userconfig")
        .arg(&user_config)
        .arg("--globalconfig")
        .arg(&global_config)
        .current_dir(folder)
        .env("npm_config_update_notifier", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_window(&mut command);
    let child = command.spawn().map_err(failed)?;
    tree.register(child.id());
    tracing::info!(package = %package.spec(), "installing the agent package");
    let output = child.output().await.map_err(failed)?;
    tree.forget();
    if output.status.success() {
        std::fs::write(&marker, package.spec()).map_err(failed)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
        Err(failed(tail.into_iter().rev().collect::<Vec<_>>().join(" ")))
    }
}

#![forbid(unsafe_code)]
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod apply;
#[cfg(windows)]
mod arguments;
#[cfg(windows)]
mod disk;
#[cfg(all(windows, test))]
mod fake_app;
#[cfg(windows)]
mod folders;
#[cfg(windows)]
mod journal;
#[cfg(windows)]
mod lock;
#[cfg(windows)]
mod message;
#[cfg(windows)]
mod notice;
#[cfg(windows)]
mod settle;
#[cfg(windows)]
mod start;
#[cfg(all(windows, test))]
mod test_install;

#[cfg(windows)]
fn main() {
    start::main();
}

// The launcher belongs to the Windows install only; elsewhere the app is started directly.
#[cfg(not(windows))]
fn main() {}

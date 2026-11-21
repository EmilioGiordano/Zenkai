use std::fs;
use std::path::Path;
use std::time::Duration;

const BEHAVIOR: &str = "fake-behavior.txt";
const STARTED_WITH: &str = "started-with.txt";
// What the launcher must pass for the copy to run only `act`.
pub const ARGUMENTS: [&str; 2] = ["fake_app::act", "--exact"];

pub enum Behavior {
    Confirm(&'static str),
    Hang,
    Exit,
}

// The test binary doubles as a fake app. Copied into an install with a behavior file beside it
// and started by the launcher, it runs only the `act` test, which does what the file says.
pub fn place(folder: &Path, behavior: Behavior) {
    fs::create_dir_all(folder).unwrap();
    fs::copy(std::env::current_exe().unwrap(), folder.join("zenkai.exe")).unwrap();
    let text = match behavior {
        Behavior::Confirm(version) => format!("confirm {version}"),
        Behavior::Hang => "hang".to_owned(),
        Behavior::Exit => "exit".to_owned(),
    };
    fs::write(folder.join(BEHAVIOR), text).unwrap();
}

// Waits for the copy started from `root\app` to write down its arguments.
pub fn started_with(root: &Path) -> Vec<String> {
    let file = root.join(STARTED_WITH);
    for _ in 0..400 {
        if let Ok(text) = fs::read_to_string(&file) {
            return text.lines().map(str::to_owned).collect();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the fake app never started");
}

#[test]
fn act() {
    let exe = std::env::current_exe().unwrap();
    let Ok(behavior) = fs::read_to_string(exe.with_file_name(BEHAVIOR)) else {
        return;
    };
    let root = exe.parent().unwrap().parent().unwrap();
    let arguments: String = std::env::args()
        .skip(1)
        .map(|argument| argument + "\n")
        .collect();
    let written = root.join("started-with.tmp");
    fs::write(&written, arguments).unwrap();
    fs::rename(written, root.join(STARTED_WITH)).unwrap();
    match behavior.split_once(' ') {
        Some(("confirm", version)) => {
            fs::write(root.join(".update").join("confirmed"), version).unwrap();
        }
        _ if behavior == "hang" => std::thread::sleep(Duration::from_secs(60)),
        _ => {}
    }
}

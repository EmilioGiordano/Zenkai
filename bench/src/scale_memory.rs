use anyhow::{Result, bail};

#[cfg(any(windows, target_os = "linux"))]
use anyhow::Context;

const MB: f64 = 1024.0 * 1024.0;

pub fn to_mb(bytes: u64) -> f64 {
    bytes as f64 / MB
}

// Committed private bytes of this process, never the working set.
pub fn current_commit_bytes() -> Result<u64> {
    #[cfg(windows)]
    {
        powershell_number(&format!(
            "(Get-Process -Id {}).PrivateMemorySize64",
            std::process::id()
        ))
    }
    #[cfg(target_os = "linux")]
    {
        linux_size_kb("Size:").map(|kb| kb << 10)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        bail!("scale stages measure committed memory on Windows and Linux only")
    }
}

pub fn peak_commit_bytes() -> Result<u64> {
    #[cfg(windows)]
    {
        // PeakPagedMemorySize64 is peak committed private bytes; the working set
        // is not used because Windows pages a process out under pressure.
        powershell_number(&format!(
            "(Get-Process -Id {}).PeakPagedMemorySize64",
            std::process::id()
        ))
    }
    #[cfg(target_os = "linux")]
    {
        // Linux has no per-process committed peak; peak resident size is the closest.
        linux_size_kb("VmHWM:").map(|kb| kb << 10)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        bail!("scale stages measure committed memory on Windows and Linux only")
    }
}

pub fn total_ram_bytes() -> Result<u64> {
    #[cfg(windows)]
    {
        powershell_number("(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory")
    }
    #[cfg(target_os = "linux")]
    {
        linux_size_kb("MemTotal:").map(|kb| kb << 10)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        bail!("scale stages measure committed memory on Windows and Linux only")
    }
}

// Heavy runs die past 60 % of physical RAM in committed bytes.
pub fn check_cap(stage: &str) -> Result<()> {
    let cap = total_ram_bytes()? * 6 / 10;
    let used = current_commit_bytes()?;
    if used > cap {
        bail!("{stage}: committed {used} bytes pass the 60 % RAM cap of {cap} bytes");
    }
    Ok(())
}

#[cfg(windows)]
fn powershell_number(script: &str) -> Result<u64> {
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("running powershell")?;
    if !output.status.success() {
        bail!("powershell failed");
    }
    String::from_utf8(output.stdout)
        .context("non UTF-8 powershell output")?
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("unparsable powershell output: {e}"))
}

#[cfg(target_os = "linux")]
fn linux_size_kb(key: &str) -> Result<u64> {
    // Same source memory-stats reads for virtual_mem: summed mapping sizes, so the
    // number matches what the app reports. VmHWM and MemTotal live in status/meminfo.
    for path in ["/proc/self/smaps", "/proc/self/status"] {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Some(kb) = text
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|number| number.parse::<u64>().ok())
        {
            return Ok(kb);
        }
    }
    if key == "MemTotal:" {
        return std::fs::read_to_string("/proc/meminfo")
            .context("reading /proc/meminfo")
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix(key))
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|number| number.parse::<u64>().ok())
                    .context("no MemTotal in /proc/meminfo")
            });
    }
    bail!("no {key} in /proc/self/smaps nor /proc/self/status");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_become_fractional_megabytes() {
        assert_eq!(to_mb(1024 * 1024), 1.0);
        assert_eq!(to_mb(0), 0.0);
    }
}

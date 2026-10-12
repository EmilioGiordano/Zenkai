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

#[cfg(any(test, target_os = "linux"))]
fn sum_sizes_kb(text: &str, key: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut found = false;
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let Some(number) = rest.split_whitespace().next() else {
            continue;
        };
        let Ok(kb) = number.parse::<u64>() else {
            continue;
        };
        total = total.saturating_add(kb);
        found = true;
    }
    found.then_some(total)
}

#[cfg(target_os = "linux")]
fn linux_size_kb(key: &str) -> Result<u64> {
    // smaps carries one Size: line per mapping; the process total is their sum,
    // the same source memory-stats reads for virtual_mem. VmHWM and MemTotal
    // appear once in status and meminfo, where a sum is just the value.
    if key == "Size:" {
        let text =
            std::fs::read_to_string("/proc/self/smaps").context("reading /proc/self/smaps")?;
        return sum_sizes_kb(&text, key).context("no Size: in /proc/self/smaps");
    }
    for path in ["/proc/self/status", "/proc/meminfo"] {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Some(kb) = sum_sizes_kb(&text, key)
        {
            return Ok(kb);
        }
    }
    bail!("no {key} in /proc/self/status nor /proc/meminfo");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_become_fractional_megabytes() {
        assert_eq!(to_mb(1024 * 1024), 1.0);
        assert_eq!(to_mb(0), 0.0);
    }

    #[test]
    fn smaps_sizes_sum_across_mappings() {
        let smaps = "00400000-00401000 r--p 00000000 00:00 0\n\
            Size: 4 kB\n\
            Rss: 4 kB\n\
            7f00-7f01 rw-p 00000000 00:00 0\n\
            Size: 1024 kB\n\
            Rss: 8 kB\n";
        assert_eq!(sum_sizes_kb(smaps, "Size:"), Some(1028));
        assert_eq!(sum_sizes_kb(smaps, "Rss:"), Some(12));
        assert_eq!(sum_sizes_kb(smaps, "VmHWM:"), None);
    }
}

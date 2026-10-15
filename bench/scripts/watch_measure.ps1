#Requires -Version 5.1
param(
    [Parameter(Mandatory = $true)][int]$TargetPid,
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][long]$LimitBytes,
    [int]$PollMs = 500
)

# Kills only the process with this exact PID, and only while its name still
# matches the measured binary. Never touches any other process.
while ($true) {
    $target = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
    if (-not $target) {
        Write-Output "watchdog: pid $TargetPid exited on its own"
        break
    }
    if ($target.ProcessName -ne $Name) {
        Write-Output "watchdog: pid $TargetPid is now '$($target.ProcessName)', not '$Name'; leaving it alone"
        break
    }
    # Committed bytes, not working set: under memory pressure the working set stays low.
    if ($target.PrivateMemorySize64 -gt $LimitBytes) {
        Stop-Process -Id $TargetPid -Force
        Write-Output "watchdog: killed $Name pid $TargetPid at committed $($target.PrivateMemorySize64) bytes (limit $LimitBytes)"
        break
    }
    Start-Sleep -Milliseconds $PollMs
}

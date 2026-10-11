$guard = Join-Path $PSScriptRoot 'guard.ps1'
$root = ''
$here = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
foreach ($line in (git -C $here worktree list --porcelain)) {
  if ($line.StartsWith('worktree ') -and $root -eq '') { $root = $line.Substring(9) }
}

$failures = 0
function Invoke-Guard($cmd, $dir) {
  $payload = @{ tool_input = @{ command = $cmd }; cwd = $dir } | ConvertTo-Json -Compress
  return ($payload | & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $guard)
}

function Check($name, $cmd, $dir, $wantDeny) {
  $out = Invoke-Guard $cmd $dir
  $denied = -not [string]::IsNullOrWhiteSpace($out) -and $out.Contains('deny')
  if ($denied -eq $wantDeny) {
    Write-Output "PASS $name"
  } else {
    Write-Output "FAIL $name (wantDeny=$wantDeny out='$out' cmd='$cmd' cwd='$dir')"
    $script:failures++
  }
}

$rootBranch = (git -C $root rev-parse --abbrev-ref HEAD).Trim()
if ($rootBranch -ne 'main') {
  Write-Output "FAIL test requires the repo root on main (is $rootBranch)"
  exit 1
}

$stagingDir = ''
$otherDir = ''
$path = ''
foreach ($line in (git worktree list --porcelain)) {
  if ($line.StartsWith('worktree ')) { $path = $line.Substring(9) }
  elseif ($line.StartsWith('branch refs/heads/')) {
    $b = $line.Substring(18)
    if ($b.StartsWith('staging/') -and $stagingDir -eq '') { $stagingDir = $path }
    elseif ($b -ne 'main' -and $otherDir -eq '') { $otherDir = $path }
  }
}
if ($stagingDir -eq '') { $stagingDir = $otherDir }
if ($stagingDir -eq '') {
  Write-Output 'FAIL no non-main worktree found to use as a merge fixture'
  exit 1
}
Write-Output "root=$root"
Write-Output "staging fixture=$stagingDir"

$merge = 'git merge --no-ff fix/guard-worktree-dir'
Check 'merge-in-staging-after-cd-allowed' "cd `"$stagingDir`" && $merge" $root $false
Check 'merge-in-staging-after-set-location-allowed' "Set-Location `"$stagingDir`"; $merge" $root $false
Check 'merge-in-staging-after-push-location-allowed' "Push-Location `"$stagingDir`" && $merge" $root $false
Check 'merge-in-staging-after-pushd-allowed' "pushd `"$stagingDir`" && $merge" $root $false
Check 'merge-in-staging-after-sl-allowed' "sl `"$stagingDir`" && $merge" $root $false
Check 'merge-in-staging-after-chdir-allowed' "chdir `"$stagingDir`" && $merge" $root $false
Check 'merge-in-staging-with-git-C-allowed' "git -C `"$stagingDir`" $merge" $root $false
Check 'merge-on-main-denied' $merge $root $true
Check 'merge-after-cd-back-to-main-denied' "cd `"$root`" && $merge" $stagingDir $true
Check 'merge-in-unknown-dir-denied' "cd `"C:\definitely-not-a-repo-9f3`" && $merge" $stagingDir $true
Check 'staging-branch-creation-allowed' 'git worktree add -b staging/2099-01-01 .claude/worktrees/staging-2099-01-01 main' $root $false
Check 'checkout-b-staging-allowed' 'git checkout -b staging/2099-01-01' $root $false
Check 'merge-abort-on-main-allowed' 'git merge --abort' $root $false

if ($failures -gt 0) { exit 1 }
Write-Output 'all guard tests passed'

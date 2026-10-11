$raw = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($raw)) { exit 0 }
try { $hook = $raw | ConvertFrom-Json } catch { exit 0 }
$command = $hook.tool_input.command
if ([string]::IsNullOrWhiteSpace($command)) { exit 0 }
$cwd = $hook.cwd

function Verdict($decision, $reason) {
  $out = @{ hookSpecificOutput = @{ hookEventName = 'PreToolUse'; permissionDecision = $decision; permissionDecisionReason = $reason } } | ConvertTo-Json -Compress
  [Console]::Out.WriteLine($out)
  exit 0
}

function OnMain($dir) {
  try {
    $b = & git -C $dir rev-parse --abbrev-ref HEAD 2>$null
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($b)) { return $true }
    return ($b.Trim() -eq 'main')
  } catch { return $true }
}

# Split compound commands; each stage is judged alone.
# One workDir across stages: an earlier cd moves the directory later git runs in.
$stages = $command -split '\r?\n|&&|\|\||;|(?<!&)&(?!&)|(?<!\|)\|(?!\|)'
$workDir = $cwd
foreach ($stage in $stages) {
  $tokens = @([regex]::Matches($stage, '"[^"]*"|''[^'']*''|\S+') | ForEach-Object { $_.Value.Trim('"', "'", '`') } | Where-Object { $_ -ne '' })
  if ($tokens.Count -eq 0) { continue }
  $lower = @($tokens | ForEach-Object { $_.ToLowerInvariant() })
  $idx = -1
  for ($k = 0; $k -lt $lower.Count; $k++) {
    if ($lower[$k] -match '^(git|gh|cargo|rm)(\.exe)?$') { $idx = $k; break }
  }
  if ($lower[0] -in @('cd', 'chdir', 'sl', 'set-location', 'pushd', 'push-location')) {
    $target = ''
    for ($j = 1; $j -lt $lower.Count; $j++) {
      if ($lower[$j].StartsWith('-')) { continue }
      $target = $lower[$j]; break
    }
    if ($target -ne '' -and $target -notmatch '^(git|gh|cargo|rm)(\.exe)?$') {
      if (-not [System.IO.Path]::IsPathRooted($target)) { $target = Join-Path $workDir $target }
      $workDir = $target
    }
  } elseif ($lower[0] -in @('pop-location', 'popd')) {
    $workDir = $cwd
  }
  if ($idx -lt 0) { continue }
  $tool = $lower[$idx] -replace '\.exe$', ''
  $rest = @()
  if ($idx + 1 -lt $lower.Count) { $rest = $lower[($idx + 1)..($lower.Count - 1)] }

  if ($tool -eq 'rm') {
    $hasR = $false; $hasF = $false
    foreach ($t in $rest) {
      if ($t -match '^-[a-zA-Z]+$' -and $t.Contains('r') -and $t.Contains('f')) {
        Verdict 'deny' 'rm with recursive and force flags is always denied.'
      }
      if ($t -in @('-r', '-R', '--recursive')) { $hasR = $true }
      if ($t -in @('-f', '--force')) { $hasF = $true }
    }
    if ($hasR -and $hasF) { Verdict 'deny' 'rm with recursive and force flags is always denied.' }
    continue
  }

  $flags = @(); $words = @()
  $skipNext = $false; $dir = $workDir
  for ($k = 0; $k -lt $rest.Count; $k++) {
    if ($skipNext) { $skipNext = $false; continue }
    $t = $rest[$k]
    if ($t.StartsWith('-')) {
      $flags += $t
      if (($t -eq '-c' -or $t -eq '-C') -and ($k + 1 -lt $rest.Count)) {
        if ($t -eq '-C') { $dir = $rest[$k + 1] }
        $skipNext = $true
      }
    } else { $words += $t }
  }

  if ($tool -eq 'gh') {
    if ($words.Count -gt 0 -and ($words[0] -eq 'pr' -or $words[0] -eq 'repo')) {
      Verdict 'deny' ('gh ' + $words[0] + ' is always denied.')
    }
    continue
  }

  if ($tool -eq 'cargo') {
    if ($words.Count -gt 0 -and @('publish', 'login', 'yank', 'owner') -contains $words[0]) {
      Verdict 'deny' ('cargo ' + $words[0] + ' is always denied.')
    }
    continue
  }

  $sub = ''
  if ($words.Count -gt 0) { $sub = $words[0] }
  switch ($sub) {
    'push' { Verdict 'deny' 'git push is always denied. The user pushes by hand.' }
    'clean' { Verdict 'deny' 'git clean is always denied.' }
    'rebase' { Verdict 'deny' 'git rebase is always denied.' }
    'branch' {
      if (($flags -contains '-d') -or ($flags -contains '-D') -or ($flags -contains '--delete')) {
        Verdict 'deny' 'Deleting branches is always denied.'
      }
    }
    'reset' {
      if ($rest -contains '--hard') { Verdict 'deny' 'git reset --hard is always denied.' }
    }
    'commit' {
      if (($rest -contains '--amend') -and (OnMain $dir)) {
        Verdict 'deny' 'git commit --amend on main is always denied.'
      }
    }
    'merge' {
      if (($rest -contains '--abort') -or ($rest -contains '--quit')) { break }
      if (OnMain $dir) { Verdict 'deny' 'Merging while main is checked out needs your approval. Ask the user first; if they approve, they run it by hand.' }
    }
    { ($_ -eq 'checkout') -or ($_ -eq 'switch') } {
      if (($flags -contains '-b') -or ($flags -contains '-B')) { break }
      if ($words.Count -gt 1 -and ($words[1] -eq 'main')) {
        Verdict 'deny' 'Checking out main needs your approval: any later merge on main needs it too. Ask the user first; if they approve, they run it by hand.'
      }
    }
    'update-ref' {
      foreach ($t in $rest) {
        if ($t.Contains('heads/main')) {
          Verdict 'deny' 'Updating the main ref needs your approval. Ask the user first; if they approve, they run it by hand.'
        }
      }
    }
  }
}
exit 0

# Runs only offline diagnostics. Extracts the current walk_config function so
# fixtures exercise production code, not a hand-maintained copy. Never opens
# an audio device or writes to Equalizer APO's configuration directory.
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$source = [IO.File]::ReadAllText((Join-Path $repo 'app/src-tauri/src/lib.rs'))
$start = $source.IndexOf('fn walk_config(')
$end = $source.IndexOf('/// One chain', $start)
if ($start -lt 0 -or $end -le $start) { throw 'walk_config source markers changed' }
$walker = $source.Substring($start, $end - $start)
$probe = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'loudness-followup-probe.rs'))
$testFile = Join-Path $repo 'app/src-tauri/tests/loudness_followup_review_temp.rs'
if (Test-Path -LiteralPath $testFile) { throw "Refusing to overwrite $testFile" }
try {
    [IO.File]::WriteAllText($testFile, $probe + [Environment]::NewLine + $walker)
    Push-Location $repo
    try {
        cargo test -p fletcher-app --test loudness_followup_review_temp -- --nocapture --test-threads=1
        if ($LASTEXITCODE -ne 0) { throw 'Diagnostic run failed' }
    } finally { Pop-Location }
} finally {
    Remove-Item -LiteralPath $testFile -ErrorAction SilentlyContinue
}

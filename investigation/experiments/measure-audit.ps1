$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixture = Join-Path $project ('target\measurement-' + [guid]::NewGuid().ToString('N'))
Copy-Item -LiteralPath (Join-Path $project 'tests\fixtures\workspace') -Destination $fixture -Recurse
$tool = Join-Path $project 'target\debug\cargo-lint-audit.exe'
$measurements = foreach ($mode in @('rustc', 'clippy')) {
    foreach ($step in @('cold', 'cached', 'source-edit')) {
        if ($step -eq 'source-edit') {
            Add-Content -LiteralPath (Join-Path $fixture 'alpha\src\main.rs') -Value "`n// Force one source rebuild for the measurement."
        }
        $auditArgs = @('--manifest-path', (Join-Path $fixture 'Cargo.toml'), '--workspace', '--all-targets', '--json', '--offline')
        if ($mode -eq 'clippy') { $auditArgs += '--clippy' }
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $raw = & $tool @auditArgs
        $timer.Stop()
        if ($LASTEXITCODE -ne 0) { throw "audit failed: $raw" }
        $report = ($raw -join "`n") | ConvertFrom-Json
        $report | ConvertTo-Json -Depth 40 | Set-Content -LiteralPath (Join-Path $fixture "$mode-$step.json") -Encoding UTF8
        [pscustomobject]@{
            mode = $mode; step = $step; total_ms = $timer.ElapsedMilliseconds
            baseline_ms = $report.baseline.milliseconds; forced_ms = $report.forced.milliseconds
            baseline_compiled = $report.baseline.compiled_selected_units; forced_compiled = $report.forced.compiled_selected_units
            baseline_all_compiled = $report.baseline.compiled_all_units; forced_all_compiled = $report.forced.compiled_all_units
            baseline_all_fresh = $report.baseline.fresh_all_units; forced_all_fresh = $report.forced.fresh_all_units
            new_diagnostics = $report.newly_observed_diagnostics.Count
            status_counts = @($report.findings | Group-Object status | ForEach-Object { [pscustomobject]@{ status = $_.Name; count = $_.Count } })
            forced_lints = $report.forced_lints
        }
    }
}
$measurements | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $project 'investigation\measurements.json') -Encoding UTF8
$measurements | Select-Object mode,step,total_ms,baseline_ms,forced_ms,baseline_compiled,forced_compiled,baseline_all_compiled,forced_all_compiled,new_diagnostics | Format-Table
"Full diagnostic snapshots: $fixture"

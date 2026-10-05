param([string]$Toolchain = 'stable')

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$outputDir = Join-Path $PSScriptRoot "results-$Toolchain"
New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
$cases = @(
    @{ Name = 'allow'; Source = '#[allow(dead_code)] fn unused() {} fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'allow-reason'; Source = '#[allow(dead_code, reason = "compat")] fn unused() {} fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'expect'; Source = '#[expect(dead_code, reason = "compat")] fn unused() {} fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'unfulfilled'; Source = '#[expect(dead_code)] fn used() {} fn main() { used(); }'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'stale'; Source = '#[allow(dead_code)] fn used() {} fn main() { used(); }'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'inner'; Source = '#![allow(dead_code)] fn unused() {} fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'multiple'; Source = '#[allow(dead_code, unused_variables)] fn unused() { let x = 1; } fn main() {}'; Flags = @('--force-warn=dead_code', '--force-warn=unused_variables') },
    @{ Name = 'group'; Source = '#[allow(unused)] fn unused() { let mut x = 1; } fn main() {}'; Flags = @('--force-warn=unused') },
    @{ Name = 'expect-group'; Source = '#[expect(unused)] fn used() { let mut x = 1; } fn main() { used(); }'; Flags = @('--force-warn=unused') },
    @{ Name = 'warnings-leaf'; Source = '#![allow(warnings)] fn unused() {} fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'warnings-group'; Source = '#![allow(warnings)] fn unused() {} fn main() {}'; Flags = @('--force-warn=warnings') },
    @{ Name = 'deny'; Source = '#[deny(dead_code)] fn unused() {} fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'forbid'; Source = '#![forbid(dead_code)] fn unused() {} fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'cap-allow'; Source = '#[allow(dead_code)] fn unused() {} fn main() {}'; Common = @('--cap-lints=allow'); Flags = @('--force-warn=dead_code') },
    @{ Name = 'nested-allows'; Source = '#[allow(dead_code)] mod nested { #[allow(dead_code)] fn unused() {} } fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'shadowed-expect'; Source = '#[expect(unused_variables)] fn used() { #[allow(unused_variables)] let x = 1; } fn main() { used(); }'; Flags = @('--force-warn=unused_variables') },
    @{ Name = 'cfg-off'; Source = '#[cfg_attr(feature = "extra", allow(dead_code))] fn unused() {} fn main() {}'; Flags = @('--force-warn=dead_code') },
    @{ Name = 'cfg-on'; Source = '#[cfg_attr(feature = "extra", allow(dead_code))] fn unused() {} fn main() {}'; Common = @('--cfg', 'feature="extra"'); Flags = @('--force-warn=dead_code') },
    @{ Name = 'allow-default'; Source = '#[allow(trivial_casts)] fn main() { let value: &u8 = &1u8; let _ = value as *const u8; }'; Flags = @('--force-warn=trivial_casts') },
    @{ Name = 'allow-default-without-attribute'; Source = 'fn main() { let value: &u8 = &1u8; let _ = value as *const u8; }'; Flags = @('--force-warn=trivial_casts') },
    @{ Name = 'unknown'; Source = '#[allow(lint_audit_nonexistent)] fn main() {}'; Flags = @('--force-warn=lint_audit_nonexistent') },
    @{ Name = 'renamed'; Source = '#[allow(unused_tuple_struct_fields)] struct Unused(u8); fn main() {}'; Flags = @('--force-warn=unused_tuple_struct_fields') },
    @{ Name = 'removed'; Source = '#[allow(private_in_public)] fn main() {}'; Flags = @('--force-warn=private_in_public') }
)
$results = foreach ($case in $cases) {
    $sourcePath = Join-Path $outputDir "$($case.Name).rs"
    Set-Content -LiteralPath $sourcePath -Value $case.Source -Encoding UTF8
    foreach ($pass in @('baseline', 'forced')) {
        $compilerArgs = @("+$Toolchain", $sourcePath, '--crate-name', 'probe', '--edition=2021', '--emit=metadata', '--error-format=json', '-o', (Join-Path $outputDir 'probe.rmeta'))
        if ($case.Common) { $compilerArgs += $case.Common }
        if ($pass -eq 'forced') { $compilerArgs += $case.Flags }
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $raw = @(& rustc @compilerArgs 2>&1 | ForEach-Object { $_.ToString() })
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        Set-Content -LiteralPath (Join-Path $outputDir "$($case.Name).$pass.jsonl") -Value $raw -Encoding UTF8
        $messages = @($raw | Where-Object { $_.StartsWith('{') } | ForEach-Object { ConvertFrom-Json -InputObject $_ })
        [pscustomobject]@{
            case = $case.Name
            pass = $pass
            exit = $exitCode
            milliseconds = $timer.ElapsedMilliseconds
            diagnostics = @($messages | Where-Object { $_.code -or $_.level -eq 'error' } | ForEach-Object {
                [pscustomobject]@{ code = $_.code.code; level = $_.level; message = $_.message; children = @($_.children | ForEach-Object { $_.message }) }
            })
        }
    }
}
$results | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $outputDir 'summary.json') -Encoding UTF8
$results | ForEach-Object {
    "$($_.case) $($_.pass) exit=$($_.exit): $((@($_.diagnostics | ForEach-Object { "$($_.code):$($_.message)" })) -join '; ')"
}

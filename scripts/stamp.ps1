param([Parameter(Mandatory)][string]$Version)

$root = Split-Path $PSScriptRoot

# tauri.conf.json — "version": "x.y.z"
$f = Join-Path $root 'src-tauri\tauri.conf.json'
$c = [IO.File]::ReadAllText($f)
$c = $c -replace '"version": "\d+\.\d+\.\d+"', ('"version": "' + $Version + '"')
[IO.File]::WriteAllText($f, $c)

# Cargo.toml — package version only (first line matching ^version = "...")
# Dependency versions use { version = "x" } syntax so this safely targets only [package].
$f = Join-Path $root 'src-tauri\Cargo.toml'
$done = $false
$lines = (Get-Content $f) | ForEach-Object {
    if (-not $done -and $_ -match '^version = "') {
        $done = $true
        'version = "' + $Version + '"'
    } else { $_ }
}
[IO.File]::WriteAllText($f, ($lines -join "`n") + "`n")

Write-Host "Stamped version $Version into tauri.conf.json and Cargo.toml"

<#
.SYNOPSIS
Builds the promochaos examples.

.DESCRIPTION
  target\debug\homegrown_cart.exe                                     command target, seeded bugs (--fixed at run time)
  examples\functions\tiered-discount-rust\dist\function.wasm          Rust Function, seeded bugs
  examples\functions\tiered-discount-rust\dist\function-fixed.wasm    Rust Function, fixed
  examples\functions\bundle-discount-js\dist\function.wasm            JS Function, seeded bugs (needs the Shopify CLI)
  examples\functions\bundle-discount-js\dist\function-fixed.wasm      JS Function, fixed (needs the Shopify CLI)

Run from anywhere. Stops on the first error.
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
$Examples = Join-Path $Root 'examples'
$Tiered = Join-Path $Examples 'functions\tiered-discount-rust'
$Bundle = Join-Path $Examples 'functions\bundle-discount-js'
$RawWasm = 'wasm32-unknown-unknown\release\tiered-discount-rust.wasm'
$Built = New-Object System.Collections.Generic.List[string]

function Test-Command([string]$Name) {
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$Directory,
        [Parameter(Mandatory = $true)][string]$Command,
        [string[]]$Arguments = @()
    )
    Push-Location $Directory
    try {
        & $Command @Arguments
        if ($LASTEXITCODE -ne 0) {
            throw "$Command $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

# Wasm API Functions need Shopify's trampoline step before function-runner can
# run them. The Shopify CLI does it during `shopify app function build`.
function Invoke-Trampoline([string]$In, [string]$Out) {
    if (Test-Command 'shopify-function-trampoline') {
        & shopify-function-trampoline -i $In -o $Out
        if ($LASTEXITCODE -ne 0) {
            throw "shopify-function-trampoline failed with exit code $LASTEXITCODE"
        }
    }
    else {
        Copy-Item -LiteralPath $In -Destination $Out -Force
        Write-Warning ("shopify-function-trampoline is not on the PATH, so $Out was copied without " +
            "the trampoline step and function-runner may refuse to run it. Install the Shopify CLI, " +
            "or the trampoline tool from the Shopify/shopify-function-wasm-api releases, and build again.")
    }
}

# schema.graphql is not committed (we found no licence that allows
# redistributing it). Stop with the fetch command when it is missing.
function Assert-Schema([string]$Name) {
    if (-not (Test-Path -LiteralPath (Join-Path $Examples "functions\$Name\schema.graphql"))) {
        Write-Host "Missing examples\functions\$Name\schema.graphql. Fetch it (needs a Shopify login): cd examples; shopify app function schema --path functions/$Name --client-id <your app client ID>"
        exit 1
    }
}

function Set-BundleIndex([string]$Line) {
    [System.IO.File]::WriteAllText((Join-Path $Bundle 'src\index.js'), "$Line`n")
}

Write-Host '== homegrown-cart'
Invoke-Native -Directory $Root -Command 'cargo' -Arguments @('build', '-p', 'homegrown-cart')
$Built.Add('target\debug\homegrown_cart.exe')

Write-Host '== tiered-discount-rust'
Assert-Schema 'tiered-discount-rust'
if (Test-Command 'rustup') {
    $installed = @(& rustup target list --installed)
    if ($installed -notcontains 'wasm32-unknown-unknown') {
        throw 'The wasm32-unknown-unknown target is missing. Run: rustup target add wasm32-unknown-unknown'
    }
}
New-Item -ItemType Directory -Force -Path (Join-Path $Tiered 'dist') | Out-Null
# Build the seeded module exactly like the fixed one below (cargo, then the
# trampoline) whenever the trampoline is on the PATH, so the before and after
# instruction counts compare like with like. The Shopify CLI build uses its own
# trampoline and gave about 5 percent fewer instructions on 2026-11-15.
if (-not (Test-Command 'shopify-function-trampoline') -and (Test-Command 'shopify')) {
    Invoke-Native -Directory $Examples -Command 'shopify' -Arguments @('app', 'function', 'build', '--path', 'functions/tiered-discount-rust')
    Copy-Item -LiteralPath (Join-Path $Tiered "target\$RawWasm") -Destination (Join-Path $Tiered 'dist\function.wasm') -Force
}
else {
    Write-Host 'Building with cargo and the trampoline.'
    Invoke-Native -Directory $Tiered -Command 'cargo' -Arguments @('build', '--target=wasm32-unknown-unknown', '--release')
    Invoke-Trampoline (Join-Path $Tiered "target\$RawWasm") (Join-Path $Tiered 'dist\function.wasm')
}
$Built.Add('examples\functions\tiered-discount-rust\dist\function.wasm')

Invoke-Native -Directory $Tiered -Command 'cargo' -Arguments @('build', '--target=wasm32-unknown-unknown', '--release', '--features', 'fixed', '--target-dir', 'target-fixed')
Invoke-Trampoline (Join-Path $Tiered "target-fixed\$RawWasm") (Join-Path $Tiered 'dist\function-fixed.wasm')
$Built.Add('examples\functions\tiered-discount-rust\dist\function-fixed.wasm')

Write-Host '== bundle-discount-js'
if (Test-Command 'shopify') {
    Assert-Schema 'bundle-discount-js'
    if (-not (Test-Path -LiteralPath (Join-Path $Bundle 'node_modules'))) {
        Invoke-Native -Directory $Bundle -Command 'npm' -Arguments @('install')
    }
    try {
        Set-BundleIndex "export * from './cart_lines_discounts_generate_run.fixed';"
        Invoke-Native -Directory $Examples -Command 'shopify' -Arguments @('app', 'function', 'build', '--path', 'functions/bundle-discount-js')
        Copy-Item -LiteralPath (Join-Path $Bundle 'dist\function.wasm') -Destination (Join-Path $Bundle 'dist\function-fixed.wasm') -Force
    }
    finally {
        Set-BundleIndex "export * from './cart_lines_discounts_generate_run';"
    }
    Invoke-Native -Directory $Examples -Command 'shopify' -Arguments @('app', 'function', 'build', '--path', 'functions/bundle-discount-js')
    $Built.Add('examples\functions\bundle-discount-js\dist\function-fixed.wasm')
    $Built.Add('examples\functions\bundle-discount-js\dist\function.wasm')
}
else {
    Write-Host 'Skipped: JavaScript Functions are compiled by the Shopify CLI (Javy), which is not on the PATH.'
    Write-Host 'To build them later:'
    Write-Host '  npm install -g @shopify/cli'
    Write-Host '  cd examples'
    Write-Host '  shopify app function schema --path functions/bundle-discount-js --client-id <your app client ID>   # needs a Shopify login'
    Write-Host '  cd functions\bundle-discount-js; npm install; cd ..\..'
    Write-Host '  shopify app function build --path functions/bundle-discount-js        # buggy, dist\function.wasm'
    Write-Host '  copy src\index.fixed.js over src\index.js, build again, rename dist\function.wasm'
    Write-Host '  to dist\function-fixed.wasm, restore src\index.js and build once more'
}

Write-Host ''
Write-Host 'Built:'
foreach ($item in $Built) {
    Write-Host "  $item"
}

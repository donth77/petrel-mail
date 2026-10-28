# One command from a clean tree to signed Windows installers and an updater manifest.
#
# The macOS sibling is scripts/release.sh. This does the same job with the
# tools Windows actually has: WiX and NSIS via `cargo tauri build`, Authenticode
# via signtool, and the same minisign key the updater already trusts.
#
# What this needs from you, once:
#
#   * A code-signing certificate. Either a thumbprint already in the
#     certificate store:
#         $env:PETREL_AUTHENTICODE_THUMBPRINT = '<thumbprint>'
#     or a PFX:
#         $env:PETREL_AUTHENTICODE_PFX = 'C:\path\petrel.pfx'
#         $env:PETREL_AUTHENTICODE_PFX_PASSWORD = '<password>'
#     The thumbprint is looked up in Current User\My. Certificates that live
#     in Local Machine need PETREL_AUTHENTICODE_MACHINE=1 as well.
#     PETREL_NO_AUTHENTICODE=1 skips this. SmartScreen will warn on the
#     result. That switch is for trying the script, not for shipping.
#   * The update-signing key release.sh already uses, at
#     $env:USERPROFILE\.config\petrel\updater.key, or wherever
#     TAURI_SIGNING_PRIVATE_KEY_PATH points. The private key never enters
#     this repository. PETREL_NO_UPDATE_ARTIFACT=1 says the omission is
#     intended: existing installs will not see the release.
#
#   scripts\release.bat 1.0.0
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$Version,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = 'Stop'

function die([string]$Message) {
    [Console]::Error.WriteLine("release: $Message")
    exit 1
}

function warn([string]$Message) {
    [Console]::Error.WriteLine("warning: $Message")
}

function say([string]$Message) {
    Write-Host ''
    Write-Host "== $Message"
}

# Panic paths must not name this machine. RUSTFLAGS splits on spaces, and a
# Windows profile path usually contains one, so the flag goes through the
# unit-separator form cargo actually keeps intact.
function Set-RemapRustflags {
    $home = $env:USERPROFILE
    if (-not $home) {
        die "USERPROFILE is not set."
    }
    $flag = "--remap-path-prefix=${home}=~"
    $sep = [char]31
    if ($env:CARGO_ENCODED_RUSTFLAGS) {
        $env:CARGO_ENCODED_RUSTFLAGS = $flag + $sep + $env:CARGO_ENCODED_RUSTFLAGS
        return
    }
    $parts = New-Object System.Collections.Generic.List[string]
    $parts.Add($flag) | Out-Null
    if ($env:RUSTFLAGS) {
        foreach ($token in ($env:RUSTFLAGS -split ' ')) {
            if ($token) { $parts.Add($token) | Out-Null }
        }
        Remove-Item Env:\RUSTFLAGS
    }
    $env:CARGO_ENCODED_RUSTFLAGS = ($parts -join $sep)
}

function Find-Signtool {
    $onPath = Get-Command signtool -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }

    $roots = @()
    if (${env:ProgramFiles(x86)}) {
        $roots += Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    }
    if ($env:ProgramFiles) {
        $roots += Join-Path $env:ProgramFiles 'Windows Kits\10\bin'
    }
    foreach ($rootBin in $roots) {
        if (-not (Test-Path -LiteralPath $rootBin)) { continue }
        $versions = Get-ChildItem -LiteralPath $rootBin -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^10\.' } |
            Sort-Object Name -Descending
        foreach ($dir in $versions) {
            $candidate = Join-Path $dir.FullName 'x64\signtool.exe'
            if (Test-Path -LiteralPath $candidate) { return $candidate }
        }
    }
    return $null
}

function Read-Manifest([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    try {
        $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
        return ($raw | ConvertFrom-Json)
    } catch {
        die "could not read $Path as JSON. $($_.Exception.Message)"
    }
}

function Assert-ManifestVersion([string]$Path) {
    $existing = Read-Manifest $Path
    if ($null -eq $existing) { return }
    if ($existing -isnot [System.Management.Automation.PSCustomObject]) {
        die "$Path is not a JSON object."
    }
    $prev = [string]$existing.version
    if ($prev -ne $Version) {
        die "latest.json says `"$prev`", this release is `"$Version`". Refusing to mix platform entries from two versions."
    }
}

# signtool appends the certificate to the file. The updater signs the bytes
# it will download, so Authenticode has to happen first.
function Sign-Authenticode([string]$Path, [string]$Signtool, [string]$TimestampUrl, [string]$Thumbprint, [string]$Pfx) {
    $signArgs = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/tr', $TimestampUrl)
    if ($Thumbprint) {
        if ($env:PETREL_AUTHENTICODE_MACHINE -eq '1') {
            $signArgs += '/sm'
        }
        $signArgs += @('/sha1', ($Thumbprint -replace '\s', ''))
    } else {
        $signArgs += @('/f', $Pfx, '/p', $env:PETREL_AUTHENTICODE_PFX_PASSWORD)
    }
    $signArgs += $Path
    & $Signtool @signArgs
    if ($LASTEXITCODE -ne 0) {
        die "signtool failed on $Path ($LASTEXITCODE)."
    }
}

if ($env:OS -ne 'Windows_NT') {
    die "release.ps1 builds a Windows installer. On macOS run scripts/release.sh."
}

if ($Rest -and $Rest.Count -gt 0) {
    [Console]::Error.WriteLine("usage: scripts\release.bat <version>   e.g. scripts\release.bat 1.0.0")
    exit 2
}
if (-not $Version) {
    [Console]::Error.WriteLine("usage: scripts\release.bat <version>   e.g. scripts\release.bat 1.0.0")
    exit 2
}

$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
$tauriDir = Join-Path $root 'apps\desktop\src-tauri'
$confPath = Join-Path $tauriDir 'tauri.conf.json'
$cargoPath = Join-Path $root 'Cargo.toml'
$changelogPath = Join-Path $root 'CHANGELOG.md'

if ($env:CARGO_TARGET_DIR) {
    if ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        $targetRoot = $env:CARGO_TARGET_DIR
    } else {
        # cargo resolves a relative CARGO_TARGET_DIR from the directory it is
        # started in, which for this script is the Tauri crate.
        $targetRoot = Join-Path $tauriDir $env:CARGO_TARGET_DIR
    }
} else {
    $targetRoot = Join-Path $root 'target'
}
# Same path scripts/release.sh writes. The installers may follow
# CARGO_TARGET_DIR, but the manifest has to stay where the other script
# looks, or the two of them never see each other's platform keys.
$manifestDir = Join-Path $root 'target\release'
$manifestPath = Join-Path $manifestDir 'latest.json'

# ---------------------------------------------------------------- preflight
say "preflight"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    die "cargo is not on PATH."
}
if (-not (Get-Command cargo-tauri -ErrorAction SilentlyContinue)) {
    die "cargo-tauri is not installed (cargo install tauri-cli --version '^2' --locked)."
}
if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    die "pnpm is not on PATH. The Tauri build embeds the UI before it compiles."
}

$rustcV = & rustc -vV
if ($LASTEXITCODE -ne 0) {
    die "rustc -vV failed ($LASTEXITCODE)."
}
$hostTriple = $null
foreach ($line in $rustcV) {
    if ($line -like 'host: *') {
        $hostTriple = $line.Substring(6).Trim()
    }
}
# One installer, one manifest key. An ARM64 or 32-bit host would write a
# different filename and a different updater target, and this script would
# then advertise it as windows-x86_64.
if ($hostTriple -ne 'x86_64-pc-windows-msvc') {
    die "this script writes a windows-x86_64 installer. rustc's host is '$hostTriple'."
}
Write-Host "host: $hostTriple"

$skipAuthenticode = $env:PETREL_NO_AUTHENTICODE -eq '1'
$thumbprint = $env:PETREL_AUTHENTICODE_THUMBPRINT
$pfx = $env:PETREL_AUTHENTICODE_PFX
$signtool = $null
$timestampUrl = $env:PETREL_TIMESTAMP_URL
if (-not $timestampUrl) {
    $timestampUrl = 'http://timestamp.digicert.com'
}
if ($skipAuthenticode) {
    warn "PETREL_NO_AUTHENTICODE=1: installers will be unsigned. SmartScreen will warn. This is for trying the script, not for shipping."
} else {
    if ($thumbprint -and $pfx) {
        die "set only one of PETREL_AUTHENTICODE_THUMBPRINT and PETREL_AUTHENTICODE_PFX."
    }
    if (-not $thumbprint -and -not $pfx) {
        die @"
no Authenticode certificate.
  Set PETREL_AUTHENTICODE_THUMBPRINT to a certificate in Current User\My,
  or PETREL_AUTHENTICODE_PFX and PETREL_AUTHENTICODE_PFX_PASSWORD.
  PETREL_AUTHENTICODE_MACHINE=1 selects Local Machine instead of Current User.
  To try the script without signing: PETREL_NO_AUTHENTICODE=1
"@
    }
    if ($pfx -and -not (Test-Path -LiteralPath $pfx)) {
        die "Authenticode PFX not found at $pfx."
    }
    if ($pfx -and [string]::IsNullOrEmpty($env:PETREL_AUTHENTICODE_PFX_PASSWORD)) {
        die "PETREL_AUTHENTICODE_PFX is set but PETREL_AUTHENTICODE_PFX_PASSWORD is empty."
    }
    $signtool = Find-Signtool
    if (-not $signtool) {
        die "signtool.exe was not found. Install the Windows SDK signing tools, or put signtool on PATH."
    }
    if ($thumbprint) {
        $where = 'Current User'
        if ($env:PETREL_AUTHENTICODE_MACHINE -eq '1') { $where = 'Local Machine' }
        Write-Host "authenticode: thumbprint in $where"
    } else {
        Write-Host "authenticode: $pfx"
    }
    Write-Host "signtool: $signtool"
}

$skipUpdate = $env:PETREL_NO_UPDATE_ARTIFACT -eq '1'
$keyPath = $env:TAURI_SIGNING_PRIVATE_KEY_PATH
if (-not $keyPath) {
    $keyPath = Join-Path $env:USERPROFILE '.config\petrel\updater.key'
}
if (-not $skipUpdate) {
    if (-not (Test-Path -LiteralPath $keyPath)) {
        die @"
no update-signing key at $keyPath.
  Generate one with:  cargo tauri signer generate -w $keyPath
  and put the printed public key in tauri.conf.json under plugins.updater.
  To ship without an update artifact on purpose: PETREL_NO_UPDATE_ARTIFACT=1
"@
    }
    Write-Host "update key: $keyPath"
}

if (-not (Test-Path -LiteralPath $confPath)) {
    die "missing $confPath"
}
$conf = Get-Content -LiteralPath $confPath -Raw -Encoding UTF8 | ConvertFrom-Json
$confVersion = [string]$conf.version
if ($confVersion -ne $Version) {
    die ("version mismatch.`n  $confPath says `"$confVersion`", you asked for `"$Version`".`n  Set the version in tauri.conf.json and commit it, then run this again:`n    `"version`": `"$Version`"")
}
$product = [string]$conf.productName
if (-not $product) {
    die "$confPath has no productName."
}

$cargoVersion = $null
$inPackage = $false
foreach ($line in (Get-Content -LiteralPath $cargoPath)) {
    if ($line -match '^\[workspace\.package\]') { $inPackage = $true; continue }
    if ($inPackage -and $line -match '^\[') { break }
    if ($inPackage -and $line -match '^version = "([^"]+)"') {
        $cargoVersion = $Matches[1]
        break
    }
}
if ($cargoVersion -ne $Version) {
    die "version mismatch.`n  Cargo.toml [workspace.package] says `"$cargoVersion`", you asked for `"$Version`"."
}

$changelog = Get-Content -LiteralPath $changelogPath -Raw -Encoding UTF8
$changelogPattern = '(?m)^## ' + [regex]::Escape($Version) + '\b'
if ($changelog -notmatch $changelogPattern) {
    die "CHANGELOG.md has no `"## $Version`" section."
}
Write-Host "version:  $Version"

# WiX names the MSI with the configured language, and the default when
# tauri.conf.json sets none is en-US. A list or a map of languages produces
# more than one MSI. This script ships one.
$language = 'en-US'
$wixLanguage = $conf.bundle.windows.wix.language
if ($wixLanguage -is [string] -and $wixLanguage) {
    $language = $wixLanguage
} elseif ($null -ne $wixLanguage -and $wixLanguage -ne '') {
    die "tauri.conf.json sets more than one WiX language. This script signs a single MSI."
}

$nsisName = "${product}_${Version}_x64-setup.exe"
$msiName = "${product}_${Version}_x64_${language}.msi"
$nsisPath = Join-Path $targetRoot "release\bundle\nsis\$nsisName"
$msiPath = Join-Path $targetRoot "release\bundle\msi\$msiName"

$dirty = & git status --porcelain
if ($LASTEXITCODE -ne 0) {
    die "git status failed ($LASTEXITCODE)."
}
if ($dirty) {
    if ($env:PETREL_ALLOW_DIRTY -eq '1') {
        warn "working tree is dirty. This build will not be reproducible."
    } else {
        die "working tree is dirty. Commit or stash first (PETREL_ALLOW_DIRTY=1 overrides)."
    }
}

$commit = (& git rev-parse HEAD)
if ($LASTEXITCODE -ne 0) {
    die "git rev-parse failed ($LASTEXITCODE)."
}
$commit = "$commit".Trim()
Write-Host "commit: $($commit.Substring(0, 7))"

# Checked before the build. A latest.json left by another version must not
# receive this release's Windows entry, and finding that out after the
# compile is the failure this script exists to avoid.
Assert-ManifestVersion $manifestPath

# The notes are compiled into the binary. An empty value is a release whose
# Updates pane and manifest both say nothing, which is only a problem if it
# was accidental.
if (-not $env:PETREL_RELEASE_NOTES) {
    warn "PETREL_RELEASE_NOTES is empty. This build will show no notes in Settings > Updates, and the manifest will carry none."
}

# ------------------------------------------------------------------- build
say "build"
Set-RemapRustflags
# custom-protocol is what the Tauri CLI turns on for a production build.
# Without it the webview goes looking for a dev server that is not there.
# beforeBuildCommand embeds the UI, so this script does not run pnpm itself.
Push-Location -LiteralPath $tauriDir
& cargo tauri build --bundles nsis,msi
$buildCode = $LASTEXITCODE
Pop-Location
if ($buildCode -ne 0) {
    die "cargo tauri build failed ($buildCode)."
}

if (-not (Test-Path -LiteralPath $nsisPath)) {
    die "NSIS installer was not produced at $nsisPath."
}
if (-not (Test-Path -LiteralPath $msiPath)) {
    die "MSI installer was not produced at $msiPath."
}

# --------------------------------------------------------------------- sign
say "sign"
if (-not $skipAuthenticode) {
    Sign-Authenticode $nsisPath $signtool $timestampUrl $thumbprint $pfx
    Sign-Authenticode $msiPath $signtool $timestampUrl $thumbprint $pfx
    Write-Host "signed: $nsisPath"
    Write-Host "signed: $msiPath"
}

# ------------------------------------------------------- update artifact
say "update artifact"
$wroteManifest = $false
if (-not $skipUpdate) {
    # createUpdaterArtifacts is off in tauri.conf.json, so `tauri build` does
    # not minisign anything. The plugin accepts the NSIS setup.exe itself
    # (not a zip) and looks up windows-x86_64 after the installer-specific
    # key. One entry covers an install that came from either the MSI or the
    # NSIS setup. Signing happens after Authenticode, because signtool
    # changes the bytes the signature has to cover.
    if ($null -eq $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ''
    }
    $env:TAURI_SIGNING_PRIVATE_KEY_PATH = $keyPath
    # Capturing stdout keeps the signature cargo prints off the console.
    # Piping to Out-Null would also hide it, and then $LASTEXITCODE is no
    # longer the signer's.
    $null = & cargo tauri signer sign --app-version $Version -- $nsisPath
    if ($LASTEXITCODE -ne 0) {
        die "cargo tauri signer sign failed ($LASTEXITCODE)."
    }
    # with_extension("exe.sig") on a .exe path is the same file as appending .sig.
    $sigPath = "$nsisPath.sig"
    if (-not (Test-Path -LiteralPath $sigPath)) {
        die "signer did not write $sigPath."
    }
    $signature = (Get-Content -LiteralPath $sigPath -Raw -Encoding ASCII).Trim()
    if (-not $signature) {
        die "signer wrote an empty signature at $sigPath."
    }

    $notes = $env:PETREL_RELEASE_NOTES
    if ($null -eq $notes) { $notes = '' }
    # Windows PowerShell 5.1 formats a custom pattern with the current
    # culture. ':' is then a locale time separator, and yyyy can be an
    # era year. The updater rejects the whole manifest unless pub_date
    # is RFC 3339, which would take the preserved macOS entries with it.
    $pubDate = [datetime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ', [cultureinfo]::InvariantCulture)
    $tag = "v$Version"
    $url = "https://github.com/donth77/petrel-mail/releases/download/$tag/$([uri]::EscapeDataString($nsisName))"

    $platforms = [ordered]@{}
    $existing = Read-Manifest $manifestPath
    if ($null -ne $existing) {
        if ($existing -isnot [System.Management.Automation.PSCustomObject]) {
            die "$manifestPath is not a JSON object."
        }
        $prev = [string]$existing.version
        if ($prev -ne $Version) {
            die "latest.json says `"$prev`", this release is `"$Version`". Refusing to mix platform entries from two versions."
        }
        if ($existing.platforms) {
            foreach ($prop in $existing.platforms.PSObject.Properties) {
                $platforms[$prop.Name] = $prop.Value
            }
        }
    }
    $platforms['windows-x86_64'] = [ordered]@{
        signature = $signature
        url       = $url
    }

    $doc = [ordered]@{
        version   = $Version
        notes     = $notes
        pub_date  = $pubDate
        platforms = $platforms
    }
    $json = $doc | ConvertTo-Json -Depth 6
    $utf8 = New-Object System.Text.UTF8Encoding $false
    New-Item -ItemType Directory -Force -Path $manifestDir | Out-Null
    [System.IO.File]::WriteAllText($manifestPath, $json + "`n", $utf8)
    $wroteManifest = $true
    Write-Host "signed installer: $nsisPath"
    Write-Host "manifest:         $manifestPath"

    $names = @($platforms.Keys)
    if (($names -notcontains 'darwin-aarch64') -or ($names -notcontains 'darwin-x86_64')) {
        warn "latest.json has no darwin-aarch64 and darwin-x86_64 keys. Publishing it as the latest release asset makes macOS installs report this update as unsupported. Run scripts/release.sh for $Version and let that script keep this Windows entry, or copy its latest.json here and run this script again."
    }
} else {
    warn "PETREL_NO_UPDATE_ARTIFACT=1: no update artifact. Existing installs will not see this release."
}

Write-Host ''
Write-Host "release ready:"
Write-Host "  $msiPath"
Write-Host "  $nsisPath"

if ($wroteManifest) {
    Write-Host @"

To publish, push this commit, then attach the MSI and the NSIS setup (what a person downloads) and latest.json (what existing installs read). The endpoint in tauri.conf.json points at releases/latest/download/latest.json, so the manifest must be an asset of the release marked latest, not a file in the repository.

If $tag does not exist yet:

  gh release create "$tag" --target "$commit" --latest --title "Petrel $Version" "$msiPath" "$nsisPath" "$manifestPath"

If $tag already exists, add these files to it. --clobber replaces a latest.json already on that release:

  gh release upload "$tag" "$msiPath" "$nsisPath" "$manifestPath" --clobber
"@
}

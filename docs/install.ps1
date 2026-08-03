# @license MIT
# @copyright 2026 Mickaël Canouil
# @author Mickaël Canouil
#
# minato installer for Windows.
#
#   powershell -ExecutionPolicy ByPass -c "irm https://m.canouil.dev/minato/install.ps1 | iex"
#
# Downloads the prebuilt release binary for this machine, verifies it against
# the release SHA256SUMS, and installs it onto PATH.
#
# Environment variables:
#   MINATO_VERSION             Install this version instead of the latest.
#   MINATO_INSTALL_DIR         Install here instead of the resolved default.
#   MINATO_SKIP_CHECKSUM=1     Skip SHA256 verification (not recommended).
#   MINATO_VERIFY_PROVENANCE=1 Also verify build provenance with the gh CLI.
#   MINATO_NO_MODIFY_PATH=1    Leave PATH alone and print what to add instead.
#
# Piped through `irm | iex` nothing can bind a parameter, so the environment
# variables above are how that form is configured. Run the file directly, or
# `&([scriptblock]::Create((irm <url>))) -Version 0.2.0`, to pass parameters.
#
# On macOS and Linux, use install.sh instead.

[CmdletBinding()]
param(
    [string]$Version,
    [string]$Dir,
    [switch]$NoModifyPath,
    [switch]$Help
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# Invoke-WebRequest renders a progress bar per chunk, which costs more time than
# the download itself on Windows PowerShell.
$ProgressPreference = 'SilentlyContinue'

$Repo = 'mcanouil/minato'
$BinaryName = 'minato'
$ExeName = 'minato.exe'

function Write-Plain {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
        Justification = 'An installer''s output is console text for a person to read, which is what Write-Host is for.')]
    param([string]$Message = '')
    Write-Host $Message
}

function Write-Info {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
        Justification = 'An installer''s output is console text for a person to read, which is what Write-Host is for.')]
    param([string]$Message)
    Write-Host $Message -ForegroundColor Green
}

function Write-Warn {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
        Justification = 'An installer''s output is console text for a person to read, which is what Write-Host is for.')]
    param([string]$Message)
    Write-Host $Message -ForegroundColor Yellow
}

function Write-Err {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '',
        Justification = 'An installer''s output is console text for a person to read, which is what Write-Host is for.')]
    param([string]$Message)
    Write-Host $Message -ForegroundColor Red
}

function Show-Usage {
    Write-Plain @"
minato installer

Usage:
  powershell -ExecutionPolicy ByPass -c "irm https://m.canouil.dev/minato/install.ps1 | iex"
  .\install.ps1 [-Version <version>] [-Dir <path>] [-NoModifyPath] [-Help]

Parameters:
  -Version <version>  Install this version instead of the latest.
  -Dir <path>         Install into this directory.
  -NoModifyPath       Leave PATH alone and print what to add instead.
  -Help               Show this help and exit.

Environment variables:
  MINATO_VERSION, MINATO_INSTALL_DIR, MINATO_SKIP_CHECKSUM,
  MINATO_VERIFY_PROVENANCE, MINATO_NO_MODIFY_PATH. See the script header for
  details.
"@
}

# minato publishes one archive per Rust target triple. Map the running machine
# onto the triple the release job built, so the filename lines up exactly.
function Get-Target {
    $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($architecture) {
        'X64' { return 'x86_64-pc-windows-msvc' }
        'Arm64' {
            # There is no aarch64-pc-windows-msvc archive to download yet, and
            # Windows runs the x64 one under emulation in the meantime.
            Write-Warn 'No native ARM64 build yet; installing the x64 binary, which Windows runs under emulation.'
            return 'x86_64-pc-windows-msvc'
        }
        default { throw "Unsupported architecture: $architecture. minato ships a Windows binary for x64 only." }
    }
}

function Get-InstallDir {
    param([string]$Override)

    if ($Override) { return $Override }
    if ($env:MINATO_INSTALL_DIR) { return $env:MINATO_INSTALL_DIR }
    if (-not $env:LOCALAPPDATA) {
        throw 'LOCALAPPDATA is not set, so there is no default install directory. Pass -Dir or set MINATO_INSTALL_DIR.'
    }
    return (Join-Path $env:LOCALAPPDATA 'Programs\minato')
}

function Get-LatestVersion {
    # Follow the redirect from the HTML /releases/latest to /releases/tag/<tag>.
    # Unlike api.github.com this is not rate-limited to 60 requests per hour per
    # IP, so users behind a shared address are not turned away with a 403.
    $response = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -UseBasicParsing -MaximumRedirection 5
    $base = $response.BaseResponse

    $final = ''
    if ($base.PSObject.Properties['RequestMessage']) {
        # PowerShell 7 answers with an HttpResponseMessage, which carries the
        # URI the last request was made to,
        $final = [string]$base.RequestMessage.RequestUri.AbsoluteUri
    }
    elseif ($base.PSObject.Properties['ResponseUri']) {
        # while Windows PowerShell 5.1 answers with an HttpWebResponse, which
        # names the same thing ResponseUri.
        $final = [string]$base.ResponseUri.AbsoluteUri
    }

    if ($final -match '/releases/tag/(.+)$') { return $Matches[1] }
    throw "Could not resolve the latest version. Pass -Version or see https://github.com/$Repo/releases."
}

function Save-Download {
    param([string]$Url, [string]$Path)
    Invoke-WebRequest -Uri $Url -OutFile $Path -UseBasicParsing
}

function Test-Checksum {
    param([string]$Path, [string]$ChecksumsPath, [string]$FileName)

    if (-not (Test-Path -LiteralPath $ChecksumsPath)) {
        throw 'SHA256SUMS is not available. Set MINATO_SKIP_CHECKSUM=1 to bypass.'
    }

    $expected = ''
    foreach ($line in Get-Content -LiteralPath $ChecksumsPath) {
        # sha256sum writes "<hash> *<name>" when it read the file in binary mode
        # and "<hash>  <name>" when it did not; the marker is not part of a name.
        $fields = $line -split '\s+', 2
        if ($fields.Count -lt 2) { continue }
        if ($fields[1].TrimStart('*') -eq $FileName) {
            $expected = $fields[0]
            break
        }
    }
    if (-not $expected) { throw "No checksum for $FileName in SHA256SUMS." }

    # sha256sum writes lowercase and Get-FileHash uppercase, so the comparison
    # has to be the case-insensitive one -ne already gives on strings.
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    if ($expected -ne $actual) {
        throw "Checksum verification failed.`n  Expected: $expected`n  Actual:   $actual"
    }
    Write-Info 'Checksum verified.'
}

function Test-Provenance {
    param([string]$Path)

    if (-not (Get-Command -Name 'gh' -ErrorAction SilentlyContinue)) {
        throw 'MINATO_VERIFY_PROVENANCE=1 needs the gh CLI, which is not installed.'
    }
    Write-Info 'Verifying build provenance...'
    & gh attestation verify $Path --repo $Repo
    if ($LASTEXITCODE -ne 0) { throw 'Build provenance verification failed.' }
}

function Add-UserPathEntry {
    param([string]$Directory)

    # The User scope specifically, read and written: $env:Path is the machine
    # and user values already merged, so writing that back would copy the whole
    # machine PATH into this user's own, permanently.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @()
    if ($userPath) { $entries = @($userPath -split ';' | Where-Object { $_ -ne '' }) }

    $normalised = $Directory.TrimEnd('\')
    foreach ($entry in $entries) {
        if ($entry.TrimEnd('\') -eq $normalised) { return $false }
    }

    [Environment]::SetEnvironmentVariable('Path', (@($entries) + $Directory) -join ';', 'User')
    # Usable in this session too, not only in the ones opened after it.
    $env:Path = "$env:Path;$Directory"
    return $true
}

try {
    if ($Help) {
        Show-Usage
        return
    }

    if ($PSVersionTable.PSVersion.Major -ge 6 -and -not $IsWindows) {
        throw 'This installer installs the Windows binary. On macOS or Linux, use https://m.canouil.dev/minato/install.sh instead.'
    }

    Write-Info "Installing $BinaryName..."
    Write-Plain

    $target = Get-Target

    if (-not $Version) { $Version = $env:MINATO_VERSION }
    if (-not $Version) {
        Write-Info 'Resolving the latest release...'
        $Version = Get-LatestVersion
    }
    # The tags carry no leading v; accept one anyway so a pasted v0.1.0 works.
    $Version = $Version -replace '^v', ''

    $installDir = Get-InstallDir -Override $Dir
    $modifyPath = -not ($NoModifyPath -or $env:MINATO_NO_MODIFY_PATH -eq '1')

    Write-Info "Version:           $Version"
    Write-Info "Target:            $target"
    Write-Info "Install directory: $installDir"
    Write-Plain

    $fileName = "$BinaryName-$Version-$target.zip"
    $baseUrl = "https://github.com/$Repo/releases/download/$Version"

    $tmpdir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $tmpdir -Force | Out-Null
    try {
        $archive = Join-Path $tmpdir $fileName

        Write-Info "Downloading $fileName..."
        try {
            Save-Download -Url "$baseUrl/$fileName" -Path $archive
        }
        catch {
            throw "Download failed: $($_.Exception.Message)`nSee https://github.com/$Repo/releases for available builds."
        }

        if ($env:MINATO_SKIP_CHECKSUM -eq '1') {
            Write-Warn 'Checksum verification skipped (MINATO_SKIP_CHECKSUM=1).'
        }
        else {
            $checksums = Join-Path $tmpdir 'SHA256SUMS'
            try {
                Save-Download -Url "$baseUrl/SHA256SUMS" -Path $checksums
            }
            catch {
                throw 'Could not download SHA256SUMS. Set MINATO_SKIP_CHECKSUM=1 to bypass.'
            }
            Test-Checksum -Path $archive -ChecksumsPath $checksums -FileName $fileName
        }

        if ($env:MINATO_VERIFY_PROVENANCE -eq '1') {
            Test-Provenance -Path $archive
        }

        Write-Info 'Extracting...'
        Expand-Archive -LiteralPath $archive -DestinationPath $tmpdir -Force
        $extracted = Join-Path $tmpdir $ExeName
        if (-not (Test-Path -LiteralPath $extracted)) {
            throw "The archive did not contain a $ExeName binary."
        }

        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        try {
            Move-Item -LiteralPath $extracted -Destination (Join-Path $installDir $ExeName) -Force
        }
        catch {
            # Windows holds a lock on a running executable, so an upgrade while
            # minato is open fails here rather than at the download. The type is
            # not matched on: a cmdlet error raised through $ErrorActionPreference
            # arrives wrapped, so the original exception is not what is caught.
            throw "Could not write $installDir\${ExeName}: $($_.Exception.Message)`nIf $BinaryName is running, close it and try again."
        }
    }
    finally {
        Remove-Item -LiteralPath $tmpdir -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Plain
    Write-Info "Installed $BinaryName $Version to $installDir\$ExeName."
    Write-Plain

    if ($modifyPath) {
        if (Add-UserPathEntry -Directory $installDir) {
            Write-Info "Added $installDir to your PATH. Open a new terminal for it to take effect."
            Write-Plain
        }
    }
    else {
        Write-Warn "PATH was left alone. Add the install directory to it yourself:"
        Write-Plain "  `$env:Path += `";$installDir`""
        Write-Plain
    }

    Write-Plain 'Next steps:'
    Write-Plain "  $BinaryName doctor   # Check configuration and tooling are usable"
    Write-Plain "  $BinaryName --help   # List the commands"
    Write-Plain
    Write-Plain 'See https://m.canouil.dev/minato/get-started/ to configure it.'
}
catch {
    Write-Err $_.Exception.Message
    exit 1
}

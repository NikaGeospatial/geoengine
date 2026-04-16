#Requires -Version 5.1
<#
.SYNOPSIS
    GeoEngine CLI Installer for Windows

.DESCRIPTION
    Installs the GeoEngine CLI tool on Windows.
    Supports both online download and offline installation.

.PARAMETER InstallDir
    Installation directory (default: C:\Program Files\GeoEngine)

.PARAMETER LocalBinary
    Path to local binary for offline installation

.PARAMETER Uninstall
    Remove the installed GeoEngine binary and user configuration directory

.EXAMPLE
    # Online installation
    irm https://raw.githubusercontent.com/NikaGeospatial/geoengine/main/install/install.ps1 | iex

.EXAMPLE
    # Offline installation
    .\install.ps1 -LocalBinary .\geoengine.exe
#>

[CmdletBinding(DefaultParameterSetName = "Online")]
param(
    [Parameter(ParameterSetName = "Online")]
    [Parameter(ParameterSetName = "Local")]
    [Parameter(ParameterSetName = "Uninstall")]
    [string]$InstallDir = "$env:ProgramFiles\GeoEngine",

    [Parameter(ParameterSetName = "Local")]
    [string]$LocalBinary,

    [Parameter(ParameterSetName = "Uninstall")]
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"

# Configuration
$RepoUrl = "https://github.com/NikaGeospatial/geoengine"
$BinaryName = "geoengine.exe"
$ConfigDir = "$env:USERPROFILE\.geoengine"

function Write-Info {
    param([string]$Message)
    Write-Host "==> " -ForegroundColor Blue -NoNewline
    Write-Host $Message
}

function Write-Success {
    param([string]$Message)
    Write-Host "[OK] " -ForegroundColor Green -NoNewline
    Write-Host $Message
}

function Write-Warn {
    param([string]$Message)
    Write-Host "[!] " -ForegroundColor Yellow -NoNewline
    Write-Host $Message
}

function Write-Err {
    param([string]$Message)
    Write-Host "[X] " -ForegroundColor Red -NoNewline
    Write-Host $Message
    exit 1
}

function Test-Administrator {
    $currentUser = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($currentUser)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-Dependencies {
    Write-Info "Checking dependencies..."

    # Check for Docker
    $docker = Get-Command docker -ErrorAction SilentlyContinue
    if ($docker) {
        $dockerVersion = docker --version
        Write-Success "Docker found: $dockerVersion"

        # Check if Docker is running
        try {
            docker info | Out-Null
            Write-Success "Docker daemon is running"
        }
        catch {
            Write-Warn "Docker daemon is not running. Start Docker Desktop."
        }
    }
    else {
        Write-Warn "Docker not found. GeoEngine requires Docker."
        Write-Host "  Install Docker Desktop: https://docs.docker.com/desktop/install/windows-install/"
    }

    # Check for WSL2 (recommended for GPU support)
    $wsl = Get-Command wsl -ErrorAction SilentlyContinue
    if ($wsl) {
        Write-Success "WSL2 available"
    }
    else {
        Write-Warn "WSL2 not found. WSL2 is recommended for GPU passthrough."
    }

    # Check for NVIDIA GPU
    $nvidia = Get-Command nvidia-smi -ErrorAction SilentlyContinue
    if ($nvidia) {
        Write-Success "NVIDIA GPU detected"
    }
}

function Get-Architecture {
    $arch = [System.Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITECTURE")
    switch ($arch) {
        "AMD64" { return "x86_64" }
        "ARM64" { return "aarch64" }
        default { Write-Err "Unsupported architecture: $arch" }
    }
}

function Install-FromDownload {
    Write-Info "Downloading GeoEngine..."

    $arch = Get-Architecture
    $platform = "windows-$arch"
    $downloadUrl = "$RepoUrl/releases/latest/download/geoengine-$platform.zip"

    $tempDir = Join-Path $env:TEMP "geoengine-install"
    $zipPath = Join-Path $tempDir "geoengine.zip"

    # Create temp directory
    New-Item -ItemType Directory -Force -Path $tempDir | Out-Null

    try {
        # Download
        Write-Info "Downloading from $downloadUrl..."
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $downloadUrl -OutFile $zipPath -UseBasicParsing

        # Extract
        Write-Info "Extracting..."
        Expand-Archive -Path $zipPath -DestinationPath $tempDir -Force

        # Install
        Install-Binary (Join-Path $tempDir $BinaryName)
    }
    finally {
        # Cleanup
        Remove-Item -Recurse -Force $tempDir -ErrorAction SilentlyContinue
    }
}

function Install-Binary {
    param([string]$BinaryPath)

    if (-not (Test-Path $BinaryPath)) {
        Write-Err "Binary not found: $BinaryPath"
    }

    Write-Info "Installing to $InstallDir..."

    # Create install directory
    if (-not (Test-Path $InstallDir)) {
        if (Test-Administrator) {
            New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        }
        else {
            Write-Err "Administrator privileges required to create $InstallDir. Run as Administrator."
        }
    }

    # Copy binary
    $destPath = Join-Path $InstallDir $BinaryName
    if (Test-Path $destPath) {
        # On Windows you cannot overwrite a running executable, but you can rename it.
        # Rename the currently-installed binary out of the way so the new one can be
        # placed at the expected path. Any stale .old.exe from a previous update is
        # cleaned up first (it is no longer running at that point).
        $oldPath = Join-Path $InstallDir "geoengine.old.exe"
        if (Test-Path $oldPath) {
            Remove-Item -LiteralPath $oldPath -Force -ErrorAction SilentlyContinue
        }
        Rename-Item -LiteralPath $destPath -NewName "geoengine.old.exe" -Force
    }
    Copy-Item -Path $BinaryPath -Destination $destPath -Force

    Write-Success "Installed to $destPath"

    # Add to PATH
    Add-ToPath $InstallDir
}

function Add-ToPath {
    param([string]$Directory)

    $currentPath = [Environment]::GetEnvironmentVariable("Path", "Machine")

    if ($currentPath -notlike "*$Directory*") {
        Write-Info "Adding to system PATH..."

        if (Test-Administrator) {
            $newPath = "$currentPath;$Directory"
            [Environment]::SetEnvironmentVariable("Path", $newPath, "Machine")
            Write-Success "Added to PATH. Restart your terminal to use 'geoengine' command."
        }
        else {
            Write-Warn "Run as Administrator to add to system PATH, or add manually:"
            Write-Host "  $Directory"
        }
    }
    else {
        Write-Success "Already in PATH"
    }
}

function Normalize-PathEntry {
    param([string]$PathEntry)

    if ([string]::IsNullOrWhiteSpace($PathEntry)) {
        return $null
    }

    $normalized = $PathEntry.Trim()

    if (
        ($normalized.StartsWith('"') -and $normalized.EndsWith('"')) -or
        ($normalized.StartsWith("'") -and $normalized.EndsWith("'"))
    ) {
        $normalized = $normalized.Substring(1, $normalized.Length - 2).Trim()
    }

    $trimmedSeparators = $normalized.TrimEnd('\', '/')
    if ($trimmedSeparators) {
        $normalized = $trimmedSeparators
    }

    return $normalized.ToLowerInvariant()
}

function Remove-FromPath {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([string]$Directory)

    if ([string]::IsNullOrWhiteSpace($Directory)) {
        Write-Warn "Remove-FromPath: no directory specified, skipping PATH cleanup"
        return
    }

    $currentPath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    if (-not $currentPath) {
        return
    }

    $normalizedDirectory = Normalize-PathEntry $Directory
    $pathParts = @(
        $currentPath -split ';' |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            ForEach-Object {
                [PSCustomObject]@{
                    Raw = $_.Trim()
                    Normalized = Normalize-PathEntry $_
                }
            }
    )
    $updatedParts = @(
        $pathParts |
            Where-Object { $_.Normalized -and $_.Normalized -ne $normalizedDirectory } |
            ForEach-Object { $_.Raw }
    )
    $newPath = $updatedParts -join ';'

    if ($newPath -ne $currentPath) {
        if ($PSCmdlet.ShouldProcess("Machine PATH", "Remove $Directory")) {
            $backupFile = Join-Path $env:TEMP "geoengine_path_backup_$(Get-Date -Format 'yyyyMMddHHmmss').txt"
            Set-Content -Path $backupFile -Value $currentPath -Encoding UTF8
            [Environment]::SetEnvironmentVariable("Path", $newPath, "Machine")
            Write-Success "Removed $Directory from PATH"
            Write-Info "Original PATH backed up to: $backupFile"
        }
    }
    else {
        Write-Success "PATH already clean"
    }
}

function Initialize-Config {
    Write-Info "Setting up configuration directory..."

    $dirs = @(
        $ConfigDir,
        "$ConfigDir\logs",
        "$ConfigDir\jobs"
    )

    foreach ($dir in $dirs) {
        if (-not (Test-Path $dir)) {
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
        }
    }

    Write-Success "Config directory: $ConfigDir"
}

function Uninstall-GeoEngine {
    [CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = "High")]
    param()

    if (-not (Test-Administrator)) {
        Write-Err "Administrator privileges required to uninstall from $InstallDir. Run as Administrator."
    }

    Write-Warn "This will remove the GeoEngine binary and all configuration data at $ConfigDir."

    $destPath = Join-Path $InstallDir $BinaryName
    $oldPath = Join-Path $InstallDir "geoengine.old.exe"

    if (Test-Path $destPath) {
        if ($PSCmdlet.ShouldProcess($destPath, "Remove GeoEngine binary")) {
            Remove-Item -LiteralPath $destPath -Force
            Write-Success "Removed $destPath"
        }
    }
    else {
        Write-Warn "Binary not found at $destPath"
    }

    if (Test-Path $oldPath) {
        if ($PSCmdlet.ShouldProcess($oldPath, "Remove GeoEngine backup binary")) {
            Remove-Item -LiteralPath $oldPath -Force
            Write-Success "Removed $oldPath"
        }
    }

    if (Test-Path $ConfigDir) {
        if ($PSCmdlet.ShouldProcess($ConfigDir, "Remove GeoEngine configuration directory")) {
            Remove-Item -LiteralPath $ConfigDir -Recurse -Force
            Write-Success "Removed $ConfigDir"
        }
    }
    else {
        Write-Warn "Config directory not found at $ConfigDir"
    }

    Remove-FromPath $InstallDir

    if (Test-Path $InstallDir) {
        $remaining = @(Get-ChildItem -LiteralPath $InstallDir -Force -ErrorAction SilentlyContinue)
        if ($remaining.Count -eq 0) {
            if ($PSCmdlet.ShouldProcess($InstallDir, "Remove empty install directory")) {
                Remove-Item -LiteralPath $InstallDir -Force -ErrorAction SilentlyContinue
                Write-Success "Removed empty install directory $InstallDir"
            }
        }
    }

    Write-Success "GeoEngine uninstalled"
}

function Show-Success {
    Write-Host ""
    Write-Host "+==========================================+" -ForegroundColor Green
    Write-Host "|   GeoEngine installed successfully!      |" -ForegroundColor Green
    Write-Host "+==========================================+" -ForegroundColor Green
    Write-Host ""
    Write-Host "Get started:"
    Write-Host "  geoengine --help              " -ForegroundColor Cyan -NoNewline
    Write-Host "Show all commands"
    Write-Host "  geoengine init                " -ForegroundColor Cyan -NoNewline
    Write-Host "Initialize a new worker (creates geoengine.yaml)"
    Write-Host "  geoengine build               " -ForegroundColor Cyan -NoNewline
    Write-Host "Build the Docker image for a worker"
    Write-Host "  geoengine apply               " -ForegroundColor Cyan -NoNewline
    Write-Host "Register or update a worker"
    Write-Host ""
    Write-Host "For GIS integration:"
    Write-Host "  geoengine apply               " -ForegroundColor Cyan -NoNewline
    Write-Host "Registers worker with ArcGIS Pro / QGIS (set in geoengine.yaml)"
    Write-Host ""
    Write-Host "Documentation: $RepoUrl"
    Write-Host ""
}

# Main
function Main {
    Write-Host ""
    Write-Host "GeoEngine CLI Installer" -ForegroundColor Blue
    Write-Host "========================"
    Write-Host ""

    if ($Uninstall) {
        Uninstall-GeoEngine
        return
    }

    # Check dependencies
    Test-Dependencies

    # Install
    if ($LocalBinary) {
        Write-Info "Installing from local binary..."
        Install-Binary $LocalBinary
    }
    else {
        Install-FromDownload
    }

    # Setup config
    Initialize-Config

    # Success message
    Show-Success
}

Main

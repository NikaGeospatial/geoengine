#!/usr/bin/env bash
#
# GeoEngine CLI Installer
# Supports Linux, macOS, and Windows WSL2
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/NikaGeospatial/geoengine/main/install/install.sh | bash
#
# Or for offline installation:
#   ./install.sh
#

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
REPO_URL="https://github.com/NikaGeospatial/geoengine"
BINARY_NAME="geoengine"
INSTALL_DIR="${GEOENGINE_INSTALL_DIR:-/usr/local/bin}"
CONFIG_DIR="${HOME}/.geoengine"

# Print functions
info() {
    echo -e "${BLUE}==>${NC} $1"
}

success() {
    echo -e "${GREEN}✓${NC} $1"
}

warn() {
    echo -e "${YELLOW}!${NC} $1"
}

error() {
    echo -e "${RED}✗${NC} $1"
    exit 1
}

# Detect OS and architecture
detect_platform() {
    local os arch

    case "$(uname -s)" in
        Linux*)     os="linux";;
        Darwin*)    os="darwin";;
        MINGW*|MSYS*|CYGWIN*) os="windows";;
        *)          error "Unsupported operating system: $(uname -s)";;
    esac

    case "$(uname -m)" in
        x86_64|amd64)   arch="x86_64";;
        arm64|aarch64)  arch="aarch64";;
        *)              error "Unsupported architecture: $(uname -m)";;
    esac

    echo "${os}-${arch}"
}

# Check for required dependencies
check_dependencies() {
    info "Checking dependencies..."

    # Check for Docker
    if ! command -v docker &> /dev/null; then
        warn "Docker not found. GeoEngine requires Docker to run containers."
        echo "  Install Docker: https://docs.docker.com/get-docker/"
    else
        success "Docker found: $(docker --version)"
    fi

    # Check if Docker daemon is running
    if docker info &> /dev/null; then
        success "Docker daemon is running"
    else
        warn "Docker daemon is not running. Start it before using GeoEngine."
    fi

    # Check for NVIDIA GPU support (optional)
    if command -v nvidia-smi &> /dev/null; then
        success "NVIDIA GPU detected"
        if docker info 2>/dev/null | grep -q "nvidia"; then
            success "NVIDIA Container Toolkit is configured"
        else
            warn "NVIDIA Container Toolkit may not be installed"
            echo "  For GPU support: https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/install-guide.html"
        fi
    fi
}

# Download binary from GitHub releases
download_binary() {
    local platform="$1"
    local tmp_dir

    tmp_dir=$(mktemp -d)
    trap "rm -rf $tmp_dir" EXIT

    info "Downloading GeoEngine for ${platform}..."

    # Construct download URL
    local download_url="${REPO_URL}/releases/latest/download/${BINARY_NAME}-${platform}.tar.gz"

    # Download
    if command -v curl &> /dev/null; then
        curl -fsSL "$download_url" -o "${tmp_dir}/${BINARY_NAME}.tar.gz" || {
            error "Failed to download from ${download_url}"
        }
    elif command -v wget &> /dev/null; then
        wget -q "$download_url" -O "${tmp_dir}/${BINARY_NAME}.tar.gz" || {
            error "Failed to download from ${download_url}"
        }
    else
        error "Neither curl nor wget found. Please install one of them."
    fi

    # Extract
    info "Extracting..."
    tar -xzf "${tmp_dir}/${BINARY_NAME}.tar.gz" -C "$tmp_dir"

    # Install
    info "Installing to ${INSTALL_DIR}..."
    if [ -w "$INSTALL_DIR" ]; then
        cp "${tmp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/"
        chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    else
        sudo cp "${tmp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/"
        sudo chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    fi

    success "GeoEngine installed to ${INSTALL_DIR}/${BINARY_NAME}"
}

# Install from local binary (offline mode).
# This `--local <path>` mode is also the contract used by the Rust CLI self-update
# flow in `src/cli/update.rs`, so keep the flag and single-path argument stable.
install_local() {
    local binary_path="$1"

    if [ ! -f "$binary_path" ]; then
        error "Binary not found: $binary_path"
    fi

    info "Installing from local binary..."

    if [ -w "$INSTALL_DIR" ]; then
        cp "$binary_path" "${INSTALL_DIR}/${BINARY_NAME}"
        chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    else
        sudo cp "$binary_path" "${INSTALL_DIR}/${BINARY_NAME}"
        sudo chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    fi

    success "GeoEngine installed to ${INSTALL_DIR}/${BINARY_NAME}"
}

uninstall_geoengine() {
    local auto_confirm="${1:-false}"

    info "Uninstalling GeoEngine..."

    warn "This will remove the GeoEngine binary and all configuration data at ${CONFIG_DIR}."
    if [ "$auto_confirm" != "true" ]; then
        printf "Are you sure? [y/N] "
        read -r confirm </dev/tty
        case "$confirm" in
            [yY]|[yY][eE][sS]) ;;
            *) info "Uninstall cancelled."; return ;;
        esac
    fi

    local binary_path="${INSTALL_DIR}/${BINARY_NAME}"

    if [ -e "$binary_path" ]; then
        info "Removing ${binary_path}..."
        if [ -w "$binary_path" ] || [ -w "$INSTALL_DIR" ]; then
            rm -f "$binary_path"
        else
            sudo rm -f "$binary_path"
        fi
        success "Removed ${binary_path}"
    else
        warn "Binary not found at ${binary_path}"
    fi

    if [ -d "$CONFIG_DIR" ]; then
        info "Removing ${CONFIG_DIR}..."
        if [ -w "$CONFIG_DIR" ]; then
            rm -rf "$CONFIG_DIR"
        else
            sudo rm -rf "$CONFIG_DIR"
        fi
        success "Removed ${CONFIG_DIR}"
    else
        warn "Config directory not found at ${CONFIG_DIR}"
    fi

    success "GeoEngine uninstalled"
}

# Build from source using Cargo
build_from_source() {
    info "Building from source..."

    if ! command -v cargo &> /dev/null; then
        error "Rust/Cargo not found. Install from https://rustup.rs/"
    fi

    # Clone or use existing source
    if [ -d "src" ] && [ -f "Cargo.toml" ]; then
        info "Building in current directory..."
        cargo build --release
        install_local "target/release/${BINARY_NAME}"
    else
        local tmp_dir
        tmp_dir=$(mktemp -d)
        trap "rm -rf $tmp_dir" EXIT

        info "Cloning repository..."
        git clone "$REPO_URL" "$tmp_dir"
        cd "$tmp_dir"
        cargo build --release
        install_local "target/release/${BINARY_NAME}"
    fi
}

# Create config directory
setup_config() {
    info "Setting up configuration directory..."
    mkdir -p "$CONFIG_DIR"
    mkdir -p "$CONFIG_DIR/logs"
    mkdir -p "$CONFIG_DIR/jobs"
    success "Config directory created at $CONFIG_DIR"
}

# Print post-install message
print_success() {
    echo ""
    echo -e "${GREEN}╔════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}║     GeoEngine installed successfully!      ║${NC}"
    echo -e "${GREEN}╚════════════════════════════════════════════╝${NC}"
    echo ""
    echo "Get started:"
    echo "  ${BLUE}geoengine --help${NC}              Show all commands"
    echo "  ${BLUE}geoengine project init${NC}        Create a new project"
    echo "  ${BLUE}geoengine service start${NC}       Start the proxy service"
    echo ""
    echo "For GIS integration:"
    echo "  ${BLUE}geoengine service register arcgis${NC}  Register with ArcGIS Pro"
    echo "  ${BLUE}geoengine service register qgis${NC}    Register with QGIS"
    echo ""
    echo "Documentation: ${REPO_URL}"
    echo ""
}

# Main installation logic
main() {
    echo ""
    echo -e "${BLUE}GeoEngine CLI Installer${NC}"
    echo "========================"
    echo ""

    # Parse arguments
    local mode="download"
    local local_binary=""
    local auto_confirm="false"

    while [[ $# -gt 0 ]]; do
        case $1 in
            --local)
                mode="local"
                local_binary="$2"
                shift 2
                ;;
            --source)
                mode="source"
                shift
                ;;
            --uninstall)
                mode="uninstall"
                shift
                ;;
            --yes|-y)
                auto_confirm="true"
                shift
                ;;
            --help)
                echo "Usage: install.sh [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --local <path>  Install from local binary"
                echo "  --source        Build from source (requires Rust)"
                echo "  --uninstall     Remove the installed binary and ~/.geoengine"
                echo "  --yes, -y       Skip confirmation prompts (for automated use)"
                echo "  --help          Show this help message"
                exit 0
                ;;
            *)
                shift
                ;;
        esac
    done

    # Install based on mode
    case $mode in
        uninstall)
            uninstall_geoengine "$auto_confirm"
            return
            ;;
        download)
            check_dependencies
            local platform
            platform=$(detect_platform)
            download_binary "$platform"
            ;;
        local)
            check_dependencies
            install_local "$local_binary"
            ;;
        source)
            check_dependencies
            build_from_source
            ;;
    esac

    # Setup config
    setup_config

    # Print success message
    print_success
}

# Run main
main "$@"

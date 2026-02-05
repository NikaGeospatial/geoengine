# Homebrew formula for GeoEngine
# Usage: brew install NikaGeospatial/tap/geoengine

class Geoengine < Formula
  desc "Docker-based isolated runtime manager for geospatial workloads"
  homepage "https://github.com/NikaGeospatial/geoengine"
  version "0.1.1"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/NikaGeospatial/geoengine/releases/download/v#{version}/geoengine-darwin-x86_64.tar.gz"
      sha256 "ee01938cb629a6090009d2e39c773369b1b5386dba72b65f42e8ad700d3e1100"
    end

    on_arm do
      url "https://github.com/NikaGeospatial/geoengine/releases/download/v#{version}/geoengine-darwin-aarch64.tar.gz"
      sha256 "4744721d9b7b9641651c68588a2439e8f3151695740b0c5df5afe2293b8555d0"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/NikaGeospatial/geoengine/releases/download/v#{version}/geoengine-linux-x86_64.tar.gz"
      sha256 "acb1c89a0490d8c8421fc91e3d3f186ea358bbd509725106e4610ddb17477707"
    end

    on_arm do
      url "https://github.com/NikaGeospatial/geoengine/releases/download/v#{version}/geoengine-linux-aarch64.tar.gz"
      sha256 "fb298f4991e038d935cf206574790d05cffe342f2c957dcb1ecc48a6b6f5523c"
    end
  end

  depends_on "docker" => :recommended

  def install
    bin.install "geoengine"

    # Install shell completions
    generate_completions_from_executable(bin/"geoengine", "completions")
  end

  def post_install
    # Create config directory
    (var/"geoengine").mkpath
  end

  def caveats
    <<~EOS
      GeoEngine has been installed!

      To get started:
        geoengine --help              Show all commands
        geoengine project init        Create a new project
        geoengine service start       Start the proxy service

      For GIS integration:
        geoengine service register arcgis  Register with ArcGIS Pro
        geoengine service register qgis    Register with QGIS

      Make sure Docker is running before using GeoEngine.

      For GPU support on macOS:
        - CUDA is not available on macOS
        - PyTorch will use MPS (Metal) backend automatically
    EOS
  end

  test do
    assert_match "geoengine #{version}", shell_output("#{bin}/geoengine --version")
  end
end

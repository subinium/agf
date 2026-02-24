class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, OpenCode, Pi, Kiro, and Cursor CLI"
  homepage "https://github.com/subinium/agf"
  version "0.5.3"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "00b094f2ae27181cc66d23bb5e62b294ef45925133d37ecc6dcc238b2d7150aa"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "f1e8d29b15f0486ae227e8df4c843c5ca77aea16fb4b47deac180d1255a9f4d2"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "49718cea88973b01a1b2c94fdc8bb15da666c278e1eb4a6363cdbcf17b81928d"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

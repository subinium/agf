class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, OpenCode, Pi, and Kiro"
  homepage "https://github.com/subinium/agf"
  version "0.4.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "83195c705e2153828ac352b897ffd99aca53d2792cd443934429db5d0f5d19ec"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "e0a3bda2f0a4342b22450664a6ae40f8bf3eb9819c20d374ac7ef70efc8bce65"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "06a545da4046df250930d40c28a624f7cadcb2eb725f300f9331f44bdb92d74e"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

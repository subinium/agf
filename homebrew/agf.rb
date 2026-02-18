class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, and OpenCode"
  homepage "https://github.com/subinium/agf"
  version "0.2.2"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "3c9a39625fb3002b2a9ed8ff25104a217ef18fd9b06b24d40f4f59f836e35393"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "ad7f0fef516aa96cb4907272dd468f8a849110ee010d9821a1f4eb8af0c61009"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "994a66f98c66b60ed76f960e0c295955bfa6c1be68b513db03e8fbfbe5f876bf"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

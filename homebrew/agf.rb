class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, and OpenCode"
  homepage "https://github.com/subinium/agf"
  version "0.2.2"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "8585545aeada2942a846afcea32c3ee4dcedae3d5d48e2b3ab16de2dba2217ac"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "cf811990a8c0a4569bcf709861463db12a078aae3cc9047529bd3fdedbb66259"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "4ada6ff7eae209bcea85c2af089539ab11e1b03620aafb1be4c16d7a824c8a5d"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

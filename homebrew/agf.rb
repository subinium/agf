class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, and OpenCode"
  homepage "https://github.com/subinium/agf"
  version "0.2.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "28f4ad53901a74c3f995de61eaaa7f3318bf972d573fa92e46087d504e9cc722"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "4ab076d7d81208e3d66873194da0054ce16b258860fde6b07b23dc7d65fb2689"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "0f52455c0ba37f5fe1df8ed04f5072b314520254dccf1bd0e7ed29165426af72"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

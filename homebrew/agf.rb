class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, and Cursor"
  homepage "https://github.com/subinium/agf"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "fb7e7d71d209db32ad78d5d81f733da0b481823a9711d76f895d3e98f2157779"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "630fd37184612255a338c8e4158d18ece9c9d3308ef1da2645f8642c68016454"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "ab8398c2e6479d82db1137959bae82c5f2ad134f2f289a3574c5ec712d2a843a"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

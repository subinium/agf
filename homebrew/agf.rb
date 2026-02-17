class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, and Cursor"
  homepage "https://github.com/subinium/agf"
  version "0.1.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "4ac83901b0cc10b21f4c6345bca6e032876bd8b883fc7a84fcbbd1dd1eb54efd"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "2a93e53ece9b81d349dc60d65de87e641a59125f075615d99fe214dfeb92a241"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "21f610d6fb2a1ed4851dc413cc083fb2fa467cebeafff9a86ea2abf5a7aa2168"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, OpenCode, Pi, Kiro, and Cursor CLI"
  homepage "https://github.com/subinium/agf"
  version "0.4.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "ec974ba12cfffba62cf68a96c1275e7cc45402ee0e414af138483ede034aed86"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "ba0b4cf426243233ab1379f94e7717d88c02172d3290461319d55d79311c0e0f"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "a904ed7b920194c0bcd9023e9cb7503781f4e492a5ef3b7f44182ef4f5bcdc16"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

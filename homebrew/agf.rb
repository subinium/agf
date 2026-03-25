class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, OpenCode, Pi, Kiro, and Cursor CLI"
  homepage "https://github.com/subinium/agf"
  version "0.7.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "0ac2f2d7b775e48f6e44587334a0876a4488d257d07cb06b3f6dc0cc05934798"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "b847077ee4f155426103096079c36ded7c90a897639ed74450a2d25aed024b11"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "99df3a19c6d4e081f2065089b7d1bd59c524527d79bac99721be0afa8f51464e"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

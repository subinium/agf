class Agf < Formula
  desc "AI Agent Session Finder TUI — find, resume, and manage AI coding agent sessions"
  homepage "https://github.com/subinium/agf"
  version "0.12.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "67c9d62e264bc5805f9101944c54f9de1a11ae0c74c20cb25db9449a8bbea1e1"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "a0baf7dee812b00622735679c3fe8880cce500a8cadf0c73c52b508126e68b4e"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "fd26bf8e6c57c945d98957ff137248cf450f486e53592a90b2bc5d1f04a22e8b"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

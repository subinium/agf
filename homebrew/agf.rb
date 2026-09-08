class Agf < Formula
  desc "AI Agent Session Finder TUI — find, resume, and manage AI coding agent sessions"
  homepage "https://github.com/subinium/agf"
  version "0.15.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "7c128c8beecb08806b242022e8c2596c5723479e0892d94b4e64a2a5ce22e861"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "9094b9a981ab76b274344a1514239a57978ae36e799710be79b47c9529dfbbe1"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "671f34411c49806ccc0e9f580f0c3205f6693d8e893dc114872107c60ff2645e"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "db21c06f0a288f832879828278303cafa6d8f1e967751ee7da9962a3c0c0aeeb"
    end
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code and Codex"
  homepage "https://github.com/subinium/agf"
  version "0.1.4"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "f92c9322682664c93a05aebe63f72d1ec2bb12565ef1931cf4e9842755353284"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "e87c199ebe473c7be280c77039c40a343c90c755a1d2f36e7ee4a2d4e0074932"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "7bd64368d515e3747f053d4d0160086dde0a39c30f65e1a138a78597efdc9060"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

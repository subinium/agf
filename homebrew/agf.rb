class Agf < Formula
  desc "AI Agent Session Finder TUI — find, resume, and manage AI coding agent sessions"
  homepage "https://github.com/subinium/agf"
  version "0.14.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "499a369fcc10a70a5eb6bc4a626340a81804ee3eaf9902b4d1838a0fb56c890a"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "7a41857a9c475fa493d191b8ac01d405ed56abf4014549850aa65b7ac22025a7"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "78b0aed0a8c2c1f56c1987309257d4d9fa6fda28fdf140bece1e535934ddaa1d"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "aebd613b141852d32f23e61d34948fc53a1d633a411a52b6023bbedb562c8388"
    end
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

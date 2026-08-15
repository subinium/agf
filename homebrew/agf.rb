class Agf < Formula
  desc "AI Agent Session Finder TUI — find, resume, and manage AI coding agent sessions"
  homepage "https://github.com/subinium/agf"
  version "0.14.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "0623218e27e508654c037343d46a043ee1e8e22ae1255426932f2d19d11ed49a"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "3566ac25262203155791c6d9b27c58f3876cb90e156004b9a2ea1b146b70f568"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "addfbceb61ff04d6b3c56451f0e40ab009e7ec896d1f3f171139832b4da4e780"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "3b22d41063c6ea4679c59c11936b427ab7610e72b9c5b08e28d86eadae2c74a0"
    end
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

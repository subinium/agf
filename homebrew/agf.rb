class Agf < Formula
  desc "AI Agent Session Finder TUI — find, resume, and manage AI coding agent sessions"
  homepage "https://github.com/subinium/agf"
  version "0.10.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "9a18208a2b1afc209758a0f91aed185313f5030790efd49f0d0f07955d31a468"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "db084e4202cf9ca0fe199ca9cb1f50c7cffa3abb8a1687e00a12819aec34fb04"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "da3a678be3e8496b55b66198726e97c702969aade0b1e8cbd6a8b7442d31a4c6"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

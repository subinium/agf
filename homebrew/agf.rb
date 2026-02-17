class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code, Codex, and Cursor"
  homepage "https://github.com/subinium/agf"
  version "0.1.2"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "8cd2bc46ed8937073266a0aebb4b85d8686d97c8dc517a9158feeff2cb78cb8b"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "63d681518e9bf355740ec02d551302146ec0a5a12a3ea12ef2d9afa18de88a01"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "243a7bba46611e63794197ce9b9c61d1b6f8111abf20ed988c29a3a1ecd2e9b7"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

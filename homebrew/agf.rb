class Agf < Formula
  desc "AI Agent Session Finder TUI — unified launcher for Claude Code and Codex"
  homepage "https://github.com/subinium/agf"
  version "0.1.3"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "ce1deef7ca1a1503aef101deb16dc6e42c434a075b4fe03a20cf5d6fd7cac617"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "d6201e8037876a089e498ec32b9932fa94798d85658c0ff5710628520f43dc72"
    end
  end

  on_linux do
    url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "29f08555af612c540039edbc851002465ef1f047dd4958276aba150e66bf90d9"
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

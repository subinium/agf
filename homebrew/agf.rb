class Agf < Formula
  desc "AI Agent Session Finder TUI — find, resume, and manage AI coding agent sessions"
  homepage "https://github.com/subinium/agf"
  version "0.15.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-apple-darwin.tar.gz"
      sha256 "361a93f5829b5dc9ffc1009879a0011436dfda295b3df4cfea3f5f3fd74027f8"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-apple-darwin.tar.gz"
      sha256 "5069ac3d8d9e9f9ba6044c0c1e2ff3815fa6366172a29fd846afca189b8c7313"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "2c441575efcf808562d7344d1fe1c62267fc27ab8d78a2394668af9c819ea600"
    else
      url "https://github.com/subinium/agf/releases/download/v#{version}/agf-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "e039b9278b96955378458f10b58eb840807b480dd2ddf373d4a7c832643b8896"
    end
  end

  def install
    bin.install "agf"
  end

  test do
    assert_match "agf", shell_output("#{bin}/agf --help")
  end
end

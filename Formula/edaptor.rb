# Homebrew formula for edaptor. This repository is its own tap:
#
#   brew tap oposs/edaptor https://github.com/oposs/edaptor
#   brew install edaptor
#
# The version, the four sha256 lines and the bottle block are rewritten by
# .github/workflows/release-build-local.yml once a release's files exist. The
# trailing marker comments and the BOTTLE-START/BOTTLE-END lines are what that
# rewrite matches on; do not remove them. The values below are placeholders
# until the first release built by that workflow.
class Edaptor < Formula
  desc "Schema-driven terminal editor for OpenLDAP directories"
  homepage "https://github.com/oposs/edaptor"
  version "1.8.1"
  license "MIT"

  # Without a bottle Homebrew treats this formula as a source build and
  # refuses to install on a Mac whose Command Line Tools are older than its
  # macOS, although nothing is compiled. One bottle per architecture is
  # enough: Homebrew falls back to a bottle built for an older macOS of the
  # same architecture.
  # BOTTLE-START
  bottle do
    root_url "https://github.com/oposs/edaptor/releases/download/v1.8.1"
    sha256 cellar: :any_skip_relocation, arm64_sonoma: "a72aa491886d711d95ee82f0f5dcd913f30a0860698bf5e67e9e559fb72a0334"
    sha256 cellar: :any_skip_relocation, sequoia: "44f5b4c49a2aaffd994bb7f8fdd0673ec4c797001575d318bfe27473dde2cc9f"
  end
  # BOTTLE-END

  on_macos do
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "a75db78947a82d5d7ea4c1235c8aadf475944285546f2a390c4b85f29a94af6f" # mac-arm
    end
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "4863e8edfa0b8a8b0a5ea8fb5e828859dc601397fa1d6f2eb8a1a2ffca63bbda" # mac-x86
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "f482f5124260295c3fe2d645c5936f43e884105040509dc8b861d2071f5f651d" # linux-x86
    end
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "10b349f3252db787213171beac7ba4f9bb173ce0ddc12b36321f8b7be86b9130" # linux-arm
    end
  end

  def install
    bin.install "edaptor"
    man1.install "man/edaptor.1"
    pkgshare.install "examples"
  end

  def caveats
    <<~EOS
      edaptor reads its configuration from ~/.config/edaptor/*.toml or
      /etc/edaptor/*.toml, or from the file named with --config.
      An annotated example is in:
        #{opt_pkgshare}/examples/config.toml
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/edaptor --version")
  end
end

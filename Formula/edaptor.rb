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
  version "1.7.0"
  license "MIT"

  # Without a bottle Homebrew treats this formula as a source build and
  # refuses to install on a Mac whose Command Line Tools are older than its
  # macOS, although nothing is compiled. One bottle per architecture is
  # enough: Homebrew falls back to a bottle built for an older macOS of the
  # same architecture.
  # BOTTLE-START
  # BOTTLE-END

  on_macos do
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # mac-arm
    end
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # mac-x86
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # linux-x86
    end
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000" # linux-arm
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

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
  version "1.8.0"
  license "MIT"

  # Without a bottle Homebrew treats this formula as a source build and
  # refuses to install on a Mac whose Command Line Tools are older than its
  # macOS, although nothing is compiled. One bottle per architecture is
  # enough: Homebrew falls back to a bottle built for an older macOS of the
  # same architecture.
  # BOTTLE-START
  bottle do
    root_url "https://github.com/oposs/edaptor/releases/download/v1.8.0"
    sha256 cellar: :any_skip_relocation, arm64_sonoma: "dcae5794cdba7afb1f0e3003a98b1a50a6291b00870176b4f531f6b0ea0a96ec"
    sha256 cellar: :any_skip_relocation, sequoia: "33ec9fc1e1115122d1a962ee40711cb01730c89311d450e98a1f82ac3e43a6f8"
  end
  # BOTTLE-END

  on_macos do
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "8e9346ca9da04d74c28e33ac75556e42c1e0f72fc7e21d58c382bec789c0ca78" # mac-arm
    end
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "53b8d4f28d6abe0c071c6b2ac06862216c6f5f72611c64827fb477c0b661207f" # mac-x86
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "7aa005e29e348cb054be28d5f3b343c4bb4a08afb1c2c0d8502af2e325cbdf94" # linux-x86
    end
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "8ec5c8ab22eca5eb7d55062eb38582f4692e070b5783867bfe56346c31dc1807" # linux-arm
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

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
  version "1.8.2"
  license "MIT"

  # Without a bottle Homebrew treats this formula as a source build and
  # refuses to install on a Mac whose Command Line Tools are older than its
  # macOS, although nothing is compiled. One bottle per architecture is
  # enough: Homebrew falls back to a bottle built for an older macOS of the
  # same architecture.
  # BOTTLE-START
  bottle do
    root_url "https://github.com/oposs/edaptor/releases/download/v1.8.2"
    sha256 cellar: :any_skip_relocation, arm64_sonoma: "ecae1ced87b021e82faf680be03b4caa6c902c7ab59fd61610e727a1a72b0dd0"
    sha256 cellar: :any_skip_relocation, sequoia: "2dfc5a4cfb33e53535e3db57a5ba2ceb14f20667e58251b256be97efd32274b2"
  end
  # BOTTLE-END

  on_macos do
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "fc5199d3a67379b8bd839ee434a00ca3c2a2f995623ee4605ddecc08c0fc49cd" # mac-arm
    end
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "adc118b373736090923f647cec1fca1c1a3fbe46fc62ddb58472f506dc45837f" # mac-x86
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "12fa5be506cedce382103804bc4d843608f1ca943eebca4f28cd2872cc224695" # linux-x86
    end
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "a2016a685429bd16cd062a8c6eb8f9763d1d6ba497bb80e8f024bc071083aa53" # linux-arm
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

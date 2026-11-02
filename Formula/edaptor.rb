# Homebrew formula for edaptor. This repository is its own tap.
#
# The release build rewrites the version line, the four sha256 lines by their
# marker comments and the bottle block between the BOTTLE markers. Keep the
# markers, and keep each url line directly above its sha256 line.
class Edaptor < Formula
  desc "Schema-driven terminal editor for OpenLDAP directories"
  homepage "https://github.com/oposs/edaptor"
  version "1.9.0"
  license "MIT"

  # BOTTLE-START
  bottle do
    root_url "https://github.com/oposs/edaptor/releases/download/v1.9.0"
    sha256 cellar: :any_skip_relocation, arm64_big_sur: "7573f22e4f15788ed77884c2fdb6a0651b895b442f1696c46ad2f907eac13af1"
  end
  # BOTTLE-END

  on_macos do
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "f5e9e20a2592121d696e2558de5703f2fd9c2c05e84153fa21e4476e9b2f42b0" # mac-arm
    end
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "cfc2901a2cbe4141fde99b7c19935ab6b77e9bb87b57378d191000ca0fa9edff" # mac-x86
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "a8a5bd956a319bd226e0ba3f36ca7576d676300678d1f82f0bb1242ef145e2ab" # linux-x86
    end
    on_arm do
      url "https://github.com/oposs/edaptor/releases/download/v#{version}/edaptor-#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "9ab9808659f82ad1ef72e0f862c9ece867dcffc6419820bd1218e1c34373951b" # linux-arm
    end
  end

  def install
    bin.install "edaptor"
    Dir["man/*.[1-9]"].each { |page| (man/"man#{page[-1]}").install page }
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

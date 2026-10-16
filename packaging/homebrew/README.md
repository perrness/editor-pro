# editor-pro Homebrew tap

This directory supplies the tap README and the formula template. The release
workflow generates `Formula/editor-pro.rb` with the published version, macOS
ARM64 archive URL and its actual SHA-256 checksum.

After the first release has updated `perrness/homebrew-tap`:

```sh
brew install perrness/tap/editor-pro
ep .
ep path/to/file
ep --version
brew update
brew upgrade editor-pro
```

An Apple Silicon Mac with macOS 14 or newer is required. `ep` is a symlink to the
single `editor-pro` executable. Git, ripgrep, language servers and the optional AI
CLI are installed separately when needed. Configuration belongs to the user, outside Homebrew:
`ep --config-path` prints its location. Upgrading or removing the formula leaves
that file alone. The editor has no self-updater.

Maintainers: see [release setup](https://github.com/perrness/editor-pro/blob/main/docs/RELEASING.md).

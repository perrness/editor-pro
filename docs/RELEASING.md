# Installation and releases

`editor-pro` is the only compiled executable. `ep` is a relative symlink to it.
Installation and upgrades never create or rewrite user configuration. There is
no in-editor update check or self-updater; Homebrew or Cargo handles updates.

## Development installation

From the repository root, install or update both commands without administrator
privileges:

```sh
sh tools/install-dev.sh
export PATH="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin:$PATH"
ep .
ep path/to/file
ep --version
```

The helper runs `cargo install --locked --path .` with an explicit install root,
then links `ep` to `editor-pro`. The default root is `CARGO_INSTALL_ROOT`, then
`CARGO_HOME`, then `~/.cargo`; an optional first argument chooses another root.
It refuses to replace an unrelated `ep` command. Add the PATH setting to your
shell startup file. Cargo's own `[install] root` setting is bypassed by this helper.

Plain `cargo install --path .` works from the checkout root (or `--path ..` from
a direct child directory). Cargo installs `editor-pro` only. To use the short
name, add `alias ep=editor-pro` to your shell startup file, or run this once for
the default Cargo location:

```sh
ln -s editor-pro "${CARGO_HOME:-$HOME/.cargo}/bin/ep"
```

For a custom Cargo install root, create the symlink in that root's `bin` directory.
Rerun the helper to update a development installation. `cargo uninstall editor-pro`
removes Cargo's binary; remove the `ep` symlink separately if you created it.

## User configuration

`ep --config-path` prints the optional configuration file's absolute location:

| Platform | Path |
| --- | --- |
| macOS | `~/Library/Application Support/editor-pro/config.json` |
| Linux | `$XDG_CONFIG_HOME/editor-pro/config.json`, or `~/.config/editor-pro/config.json` |

On Linux only an absolute, nonempty `XDG_CONFIG_HOME` is used. A missing config
file means defaults; if no absolute home/config directory is available, startup
also uses defaults. `--config-path` reports an error when it cannot resolve a path.
macOS uses its native Application Support directory, independently of XDG.

Create the directory and file yourself when you want a persistent setting:

```json
{
  "theme": "tokyo-night"
}
```

Supported themes are `tokyo-night`, `catppuccin`, and `gruvbox`; omitted settings
use defaults. Invalid JSON, unknown fields/themes and files over 64 KiB fail
before terminal setup and report the config path. `--help` and `--version` work
even with invalid configuration. `:theme` changes only the current session.
No configuration is read from beside the executable or from a project checkout.
The existing environment variables for optional language/AI services still apply.

## Homebrew installation and updates

After the tap has been bootstrapped and the first release workflow succeeds:

```sh
brew install perrness/tap/editor-pro
ep --version
brew update
brew upgrade editor-pro
```

The formula selects the native ARM64 or Intel archive and installs both command
names. Release binaries target macOS 14 or newer and are tested on macOS 15.
Use an existing user-writable Homebrew installation; the editor formula needs no
`sudo`. `brew uninstall editor-pro` removes the commands and keeps user settings.
Homebrew's own initial installation is separate from the editor installation.

Git, ripgrep, language servers and the optional AI CLI remain separate optional
tools. No runtime, grammar download or configuration directory is bundled beside
the executable. Linux source builds remain available; Linux release archives and
Homebrew-on-Linux installation are deferred.

## One-time maintainer setup

1. Create a public, initialized `perrness/homebrew-tap` repository. For example,
   with an authenticated GitHub CLI:

   ```sh
   gh repo create perrness/homebrew-tap --public --add-readme \
     --description 'Homebrew formulae for editor-pro'
   ```

2. Create a dedicated SSH deploy key with write access to the tap. Store its
   private key as the `HOMEBREW_TAP_DEPLOY_KEY` Actions secret in the source
   repository. For example, in Bash:

   ```sh
   tap_key_dir=$(mktemp -d)
   ssh-keygen -t ed25519 -N '' -C editor-pro-release -f "$tap_key_dir/key"
   gh api repos/perrness/homebrew-tap/keys --method POST \
     -f title=editor-pro-release -F key=@"$tap_key_dir/key.pub" -F read_only=false
   gh secret set HOMEBREW_TAP_DEPLOY_KEY --repo perrness/editor-pro < "$tap_key_dir/key"
   rm -r "$tap_key_dir"
   ```

   The default `GITHUB_TOKEN` cannot push to a different repository. The deploy
   key can write only to the tap and belongs in Actions secrets, never in Git.
3. For a different tap, set the `HOMEBREW_TAP` repository variable to `OWNER/homebrew-tap`
   and adjust the installation examples/tap README. Allow the deploy key to push to
   the tap's default branch, including any branch protection rules.
4. Enable Actions and allow the release job's `contents: write` permission in the
   source repository. The workflow refuses to start a release without tap
   credentials or when the stable tag differs from `Cargo.toml`. Rotate the key
   by replacing the secret and removing the previous key from the tap settings.

The first successful release writes `Formula/editor-pro.rb` and the tap README.
The template deliberately contains no invented release URLs or checksums for an
unpublished version; do not copy the `.rb.in` file directly into a live tap.

## Publishing a version

Update `package.version` in `Cargo.toml`, update the package entry in `Cargo.lock`
with Cargo, and commit the reviewed changes. Then push a matching stable tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

Use the version you are actually releasing. Only stable `vMAJOR.MINOR.PATCH`
tags are supported initially; prerelease tags fail validation.

The [release workflow](../.github/workflows/release.yml):

1. Checks the tag and tap access, and runs the reusable Linux/macOS CI suite.
2. Builds native `aarch64-apple-darwin` and `x86_64-apple-darwin` release binaries
   with the locked dependency set. Packaging verifies each binary's `--version`.
3. Tests the generated Homebrew formula against the local archives on both
   macOS architectures, including PTY sessions through the installed `ep` command.
4. Creates a draft GitHub release, uploads both versioned `.tar.gz` archives and
   `SHA256SUMS`, then publishes it only after all checks pass.
5. Downloads the published assets, verifies their checksums, generates the formula
   with those exact hashes, and commits it to the tap. Only the latest stable
   release updates the tap.

Each archive contains one `editor-pro` executable, an `ep` symlink and the README.
For manual installation, download the archive matching your CPU and `SHA256SUMS`
from the release page. On macOS, `shasum -a 256 ARCHIVE.tar.gz` must match the
corresponding entry. Extract it, put `editor-pro` in `~/.local/bin`, and create
`~/.local/bin/ep` as a symlink to `editor-pro`; add that directory to PATH.
Prefer Homebrew for managed upgrades. Signing/notarization is not configured.

Rerun failed jobs to recover an interrupted release. Draft assets may be replaced
before publication; a rerun preserves already published assets and regenerates
the tap formula from the downloaded release, not rebuilt tarballs. If only the
tap update fails, fix its credentials/branch policy and rerun that job. Rerunning
an older release cannot downgrade the formula while a newer release is latest.

To add Linux later, extend the release matrix, `TARGETS` in `tools/release.py`,
and the formula's platform selection together, then validate the installed
binaries on those platforms. Do not add update logic to the editor.

References: [GitHub runner labels](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
[Homebrew taps](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap), and
[formula platform blocks](https://docs.brew.sh/Formula-Cookbook#handling-different-system-configurations).
The tap uses [GitHub deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys)
through [checkout's SSH support](https://github.com/actions/checkout/blob/v4/README.md).

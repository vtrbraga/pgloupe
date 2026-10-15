# Install

pgloupe is one binary. Pick the channel that suits you; each channel updates the way that channel normally does.

> **Pre-release.** Until `0.1.0` is out, only some of these channels exist. The release notes of each pre-release say which.

| Channel | Install | Update |
|---|---|---|
| Installer script (Linux, macOS) | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/vtrbraga/pgloupe/releases/latest/download/pgloupe-installer.sh \| sh` | `pgloupe-update` |
| Installer script (Windows) | `powershell -ExecutionPolicy Bypass -c "irm https://github.com/vtrbraga/pgloupe/releases/latest/download/pgloupe-installer.ps1 \| iex"` | `pgloupe-update` |
| Homebrew | `brew install vtrbraga/tap/pgloupe` | `brew upgrade pgloupe` |
| Cargo | `cargo install pgloupe` (needs a C toolchain and `libclang`) | the same command |
| Prebuilt, through cargo-binstall | `cargo binstall pgloupe` | the same command |
| Docker | `docker run --rm -v "$PWD:/data" ghcr.io/vtrbraga/pgloupe report /data/postgresql.log` | `docker pull ghcr.io/vtrbraga/pgloupe` |
| Archive | download from the [releases page](https://github.com/vtrbraga/pgloupe/releases), unpack, put `pgloupe` on your `PATH` | download again |

Linux and macOS are tested in CI. Windows builds are experimental.

## Updating

- **Installer script:** `pgloupe-update` updates the install in place. It reads the receipt the installer left and connects to the network only when you run it.
- **Homebrew, Cargo, Docker:** use their own upgrade command (table above). `pgloupe-update` only knows about installs made by the installer.
- pgloupe never checks for a newer version on its own. Watch the [releases feed](https://github.com/vtrbraga/pgloupe/releases.atom) and the [changelog](https://github.com/vtrbraga/pgloupe/blob/main/CHANGELOG.md).

## Pin a version

Put the version in the installer URL instead of `latest`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/vtrbraga/pgloupe/releases/download/v0.1.0/pgloupe-installer.sh | sh
```

Cargo takes `cargo install pgloupe --version 0.1.0`.

## Check what you downloaded

Every archive has a `.sha256` file beside it:

```bash
sha256sum --check pgloupe-x86_64-unknown-linux-gnu.tar.gz.sha256
```

Release archives also carry a build attestation that ties them to the workflow run that built them:

```bash
gh attestation verify pgloupe-x86_64-unknown-linux-gnu.tar.gz --repo vtrbraga/pgloupe
```

## Uninstall

Delete the binary (`which pgloupe`), or use the channel's own command (`brew uninstall pgloupe`, `cargo uninstall pgloupe`). pgloupe keeps no state outside the files you ask it to write, except a store you created with `logs ingest`, which is a single file you chose the path of.

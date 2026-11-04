<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="design/logo/couple-wordmark.svg">
    <source media="(prefers-color-scheme: light)" srcset="design/logo/couple-wordmark-light.svg">
    <img alt="Couple" src="design/logo/couple-wordmark-light.svg" width="330">
  </picture>
</p>

<p align="center">Pairs and syncs two recordings of the same event by matching their audio.</p>

---

Couple pairs and syncs two independent video recordings of the same event by
matching their audio. Give it a folder of clips from two cameras (or a camera
plus an external audio recorder), and it fingerprints the audio, finds the
time offset for each pair, checks the match, and exports synced pairs.

Two shells share one Rust engine: a native macOS app (`mac/`) and a
cross-platform Electron app (`electron/`). The same engine ships as a CLI
(`couple`) for batch runs.

## Download (Electron app)

Grab the installer for your platform from the
[latest release](https://github.com/timbeccue/couple/releases/latest):

- macOS (Apple Silicon): [Couple-mac-arm64.dmg](https://github.com/timbeccue/couple/releases/latest/download/Couple-mac-arm64.dmg)
- Windows (x64): [Couple-win-x64.exe](https://github.com/timbeccue/couple/releases/latest/download/Couple-win-x64.exe)
- Linux (x64): [Couple-linux-x64.AppImage](https://github.com/timbeccue/couple/releases/latest/download/Couple-linux-x64.AppImage)
  or [Couple-linux-x64.deb](https://github.com/timbeccue/couple/releases/latest/download/Couple-linux-x64.deb)

These links always point at the newest published release. Older builds are on
the [releases page](https://github.com/timbeccue/couple/releases).

### First launch

The builds are not code-signed, so each OS warns once.

- **macOS**: drag Couple to Applications and open it. If macOS says it can't
  verify the app, open System Settings → Privacy & Security, scroll to the
  "Couple was blocked" notice and click **Open Anyway**. Or, in Terminal:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Couple.app
  ```

- **Windows**: on the SmartScreen dialog click **More info**, then **Run anyway**.
- **Linux**: make the AppImage executable and run it, or install the .deb:

  ```bash
  chmod +x Couple-linux-x64.AppImage && ./Couple-linux-x64.AppImage
  ```

  ```bash
  sudo apt install ./Couple-linux-x64.deb
  ```

### Requirements

Nothing to install: ffmpeg and ffprobe are bundled with the app on every
platform. macOS builds are Apple Silicon only for now; Windows and Linux
builds are x64.

## Build from source

See [CLAUDE.md](CLAUDE.md) for the full build and test commands. The short
version for the Electron app:

```bash
cd electron && npm ci && npm run dev
```

This needs a Rust toolchain, Node 22, and ffmpeg on your PATH.

## Cutting a release (maintainer)

Releases never happen on their own. Push a version tag and the
[release workflow](.github/workflows/release.yml) builds installers for all
three platforms and attaches them to a **draft** release; publish the draft
by hand on GitHub once the assets look right.

```bash
git tag v0.1.0 && git push origin v0.1.0
```

The tag is the version (it is stamped into `electron/package.json` at build
time; no bump commit needed). To check packaging without tagging, run the
workflow manually from the Actions tab on any branch: it builds the
installers as workflow artifacts and creates no release.

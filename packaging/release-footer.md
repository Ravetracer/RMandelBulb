
---

### Downloads

| System | File |
|--------|------|
| Linux x86_64 | `RMandelbulb3D-<version>-linux-x86_64.tar.gz` (see INSTALL.txt inside for the needed libraries) |
| Linux, any distribution | `RMandelbulb3D-<version>-x86_64.AppImage` (`chmod +x`, then run; `… cli` runs the command line tool) |
| Windows 10/11 x64 | `RMandelbulb3D-<version>-windows-x64.zip` |
| macOS, Apple silicon | `RMandelbulb3D-<version>-macos-arm64.zip` |
| macOS, Intel | `RMandelbulb3D-<version>-macos-x86_64.zip` |

The builds are not signed. On macOS remove the quarantine flag after moving the app to
/Applications: `xattr -dr com.apple.quarantine /Applications/RMandelbulb3D.app`. On Windows
SmartScreen may ask: "More info" › "Run anyway". Details in INSTALL.txt of each package.

Network rendering needs the same version on every computer.

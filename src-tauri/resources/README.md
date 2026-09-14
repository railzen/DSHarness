# Offline Windows runtime

`pnpm prepare:runtime` prepares `bundle/` on the build machine.
Versions and SHA-256 hashes are locked in `scripts/runtime/assets.json`; DSH and all
npm dependencies are locked in `scripts/runtime/package-lock.json`.
Only the build machine needs network access, Node and npm. The installer contains
expanded runtime files and does not run npm or fetch DSH dependencies on the target PC.
The desktop UI uses the system WebView2 runtime and doesn't bundle or install it.
Do not commit generated runtime binaries. Preserve their upstream license files.

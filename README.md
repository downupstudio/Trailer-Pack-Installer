# Trailer Pack Installer

Windows MSI installer for the Trailer Pack Fabric modpack. 

This tool automates the setup of a modded Minecraft instance. It installs the Fabric loader, downloads pinned mods/shaders/resource packs from Modrinth, and writes the necessary config files so Iris shaders and resource packs are active on the very first launch.

## Launcher Support
- **Official Minecraft Launcher**: Creates a custom profile pointing to an isolated `gameDir` inside `.minecraft/versions/`.
- **Crystal Launcher**: Generates a valid launcher instance folder (`u.Trailer Pack <version>`), including the `nt_static` native libraries (downloaded directly from Mojang), `.fbcmlversion`, and the required `launcher_profiles.json`.

## Architecture
- **Frontend:** Vanilla TS/HTML/CSS
- **Backend:** Rust (Tauri framework)
- **Bundler:** WiX Toolset (MSI)

## How it works
1. **Fabric Installation**: Downloads the official Fabric installer and runs it via CLI.
2. **Modrinth API**: Uses pinned version IDs from `src-tauri/pack_manifest.toml` to guarantee reproducible builds. Dependencies are resolved automatically.
3. **Config Injection**: Writes `config/iris.properties` (`enableShaders=true`, `shaderPack=...`) and `options.txt` before the first boot. Resource packs are merged and ordered correctly to ensure Bare Bones and Fresh Animations stack properly.

## Local Development
Requires Node.js, Rust, and the Visual Studio C++ Build Tools.

```bash
# Install dependencies
npm install

# Run in development mode
npm run tauri dev

# Build the MSI
npm run tauri build
```

The resulting installer will be at `src-tauri/target/release/bundle/msi/`.

## Manifest Generation
The `src-tauri/pack_manifest.toml` file contains hardcoded Modrinth version IDs. To update the mods to their latest compatible versions, run the Python script (requires `pip install requests`):

```bash
python generate_manifest.py
```

Move the generated file to `src-tauri/pack_manifest.toml` before building.

## Releases
Pushing a version tag triggers a GitHub Actions workflow that builds the MSI and attaches it to a new GitHub Release.

```bash
git tag v1.1.0
git push origin v1.1.0
```

# GeoEngine Development Setup
This section will highlight the steps to setup the development environment for GeoEngine.

## 1. Prerequisites
* Terminal/CMD with root access
  * Windows: Run as administrator
  * Linux/macOS: `sudo`
* [Docker](https://docs.docker.com/get-docker/) (required)
* [Rust Language](https://rust-lang.org/tools/install/) (required)
* Your favourite IDE with Rust support
    * [VSCode](https://code.visualstudio.com/download) (with [Rust Extension](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust))
    * [RustRover](https://www.jetbrains.com/help/rust/installation-guide.html) (JetBrains)

## 2. Cloning the Repository
```bash
git clone https://github.com/NikaGeospatial/geoengine
cd geoengine
```

## 3. Setting Up Hot-Reloading

### 3.1. Build Debug Version
```bash
cargo build
```
The debug version of the application is located in `target/debug/`.

Depending on your OS, the binary created is different.
- Windows: `target/debug/geoengine.exe`
- Linux/macOS: `target/debug/geoengine`

### 3.2. Create Symbolic Link
This creates a global shortcut that allows terminal/CMD to access the latest built debug version of GeoEngine.
#### Windows
For Windows, GeoEngine is normally installed in the "Program Files" folder. Copy the path of the `debug` folder in GeoEngine and create a symlink directly.
```bash
mklink "%programfiles%\geoengine.exe" "C:\Path\To\Debug\Folder\geoengine.exe"
```
#### Linux/macOS
For Linux/macOS, find the path of the system that sees binaries. For example, `/usr/local/bin`. Move into that folder.
```bash
cd /path/to/system/bin
```
Copy the path of the `debug` folder in GeoEngine and create a symlink.
```bash
sudo ln -s /path/to/debug/folder/geoengine geoengine
```

## 4. Developing
Any changes to the code should be followed by a `cargo build` to ensure that the changes are reflected in the application.

If you make changes to any config templates, Dockerfile generation, or plugin changes, run `geoengine patch` to update all
artifacts.

You can also chain the `build` and `patch` commands to speed up development. Use a terminal to run
```bash
cargo build && geoengine patch
```

### 4.1. Making Breaking Changes
Any changes to config templates that require additional actions in the patch,
e.g. adding a required state attribute, need to have a migratable patch action.

This can be done by registering a function in `src/cli/patch.rs`. Find the correct stage your action should
run in, register the function in the list, and then define the function. Refer to the other existing patch actions
for examples.
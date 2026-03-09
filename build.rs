use std::fs;
use std::io::Write;
use std::path::Path;

/// Walks `source_dir`, collecting every non-hidden file from every non-hidden subdirectory.
/// Emits `cargo:rerun-if-changed` for each file found and for the root directory itself.
/// Returns a vec of `(subdir_name, file_name, absolute_path_string)`, sorted for deterministic
/// output. `__pycache__` directories (and any other hidden directories) are skipped.
fn collect_embedded_files(source_dir: &Path) -> Vec<(String, String, String)> {
    let mut entries: Vec<(String, String, String)> = Vec::new();

    println!("cargo:rerun-if-changed={}", source_dir.display());

    if !source_dir.is_dir() {
        return entries;
    }

    let mut subdirs: Vec<_> = fs::read_dir(source_dir)
        .unwrap_or_else(|_| panic!("Failed to read directory: {}", source_dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();

    for subdir in subdirs {
        let subdir_name = match subdir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip hidden directories (e.g. .DS_Store, .idea, .venv) and __pycache__.
        if subdir_name.starts_with('.') || subdir_name == "__pycache__" {
            continue;
        }

        let mut files: Vec<_> = fs::read_dir(&subdir)
            .unwrap_or_else(|_| panic!("Failed to read subdirectory: {}", subdir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();

        for file in files {
            let file_name = match file.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip hidden files (e.g. .DS_Store).
            if file_name.starts_with('.') {
                continue;
            }

            println!("cargo:rerun-if-changed={}", file.display());

            let abs_path = file
                .canonicalize()
                .unwrap_or_else(|_| panic!("Failed to canonicalize path: {}", file.display()));

            entries.push((subdir_name.clone(), file_name, abs_path.to_string_lossy().into_owned()));
        }
    }

    entries
}

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");

    // -------------------------------------------------------------------------
    // skills/  →  skills_embedded.rs
    //
    // Generates:
    //   const SKILLS: &[SkillFile] = &[
    //       SkillFile { skill: "use-geoengine", file: "SKILL.md", content: include_str!("…") },
    //       …
    //   ];
    // -------------------------------------------------------------------------
    let skills_entries = collect_embedded_files(&Path::new(&manifest_dir).join("skills"));

    let skills_out = Path::new(&out_dir).join("skills_embedded.rs");
    let mut out = fs::File::create(&skills_out).expect("Failed to create skills_embedded.rs");
    writeln!(out, "const SKILLS: &[SkillFile] = &[").unwrap();
    for (skill, file, path) in &skills_entries {
        writeln!(
            out,
            "    SkillFile {{ skill: {skill:?}, file: {file:?}, content: include_str!({path:?}) }},",
        ).unwrap();
    }
    writeln!(out, "];").unwrap();

    // -------------------------------------------------------------------------
    // plugins/  →  plugins_embedded.rs
    //
    // Each subdirectory of plugins/ is one plugin (e.g. "qgis-ge", "arcgis-ge").
    // Generates:
    //   const PLUGIN_FILES: &[PluginFile] = &[
    //       PluginFile { plugin: "arcgis-ge", file: "GeoEngineTools.pyt", content: include_str!("…") },
    //       …
    //   ];
    // -------------------------------------------------------------------------
    let plugin_entries = collect_embedded_files(&Path::new(&manifest_dir).join("plugins"));

    let plugins_out = Path::new(&out_dir).join("plugins_embedded.rs");
    let mut out = fs::File::create(&plugins_out).expect("Failed to create plugins_embedded.rs");
    writeln!(out, "const PLUGIN_FILES: &[PluginFile] = &[").unwrap();
    for (plugin, file, path) in &plugin_entries {
        writeln!(
            out,
            "    PluginFile {{ plugin: {plugin:?}, file: {file:?}, content: include_str!({path:?}) }},",
        ).unwrap();
    }
    writeln!(out, "];").unwrap();
}

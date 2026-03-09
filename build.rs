use std::fs;
use std::io::Write;
use std::path::Path;

// =================================================================================================
// Shared helpers
// =================================================================================================

/// Recursively collects every non-hidden file under `dir`.
/// Emits `cargo:rerun-if-changed` for each file and directory visited.
/// Returns a sorted vec of `(relative_path_from_dir, absolute_path_string)`.
/// Hidden entries (names starting with `.`) and `__pycache__` directories are skipped.
fn collect_files_recursive(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|_| panic!("Failed to read directory: {}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();

    for entry in entries {
        let name = match entry.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip hidden entries and __pycache__.
        if name.starts_with('.') || name == "__pycache__" {
            continue;
        }

        if entry.is_dir() {
            collect_files_recursive(&entry, base, out);
        } else if entry.is_file() {
            println!("cargo:rerun-if-changed={}", entry.display());

            let rel = entry
                .strip_prefix(base)
                .unwrap_or_else(|_| panic!("Failed to strip prefix from {}", entry.display()))
                .to_str()
                .unwrap_or_else(|| panic!("Non-UTF-8 path: {}", entry.display()))
                .to_string();

            let abs = entry
                .canonicalize()
                .unwrap_or_else(|_| panic!("Failed to canonicalize: {}", entry.display()))
                .to_string_lossy()
                .into_owned();

            out.push((rel, abs));
        }
    }
}

/// Walks the immediate subdirectories of `parent_dir`.
/// For each non-hidden subdirectory, recursively collects all files within it.
/// Returns a sorted vec of `(subdir_name, Vec<(relative_path, absolute_path)>)`.
fn collect_entries_by_subdir(parent_dir: &Path) -> Vec<(String, Vec<(String, String)>)> {
    println!("cargo:rerun-if-changed={}", parent_dir.display());

    if !parent_dir.is_dir() {
        return Vec::new();
    }

    let mut subdirs: Vec<_> = fs::read_dir(parent_dir)
        .unwrap_or_else(|_| panic!("Failed to read directory: {}", parent_dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();

    let mut result = Vec::new();

    for subdir in subdirs {
        let subdir_name = match subdir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip hidden directories and __pycache__.
        if subdir_name.starts_with('.') || subdir_name == "__pycache__" {
            continue;
        }

        let mut files: Vec<(String, String)> = Vec::new();
        collect_files_recursive(&subdir, &subdir, &mut files);

        result.push((subdir_name, files));
    }

    result
}

// =================================================================================================
// main
// =================================================================================================

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");

    // -------------------------------------------------------------------------
    // skills/  →  skills_embedded.rs
    //
    // Generates one SkillEntry per skill subdirectory.  All files within the
    // subdirectory (recursively) are embedded as an inline `&[(&str, &str)]`
    // of (relative_path, content) pairs so that nested asset directories are
    // supported automatically.
    //
    //   const SKILLS: &[SkillEntry] = &[
    //       SkillEntry {
    //           skill: "use-geoengine",
    //           files: &[
    //               ("SKILL.md", include_str!("…/SKILL.md")),
    //               ("assets/diagram.svg", include_str!("…/assets/diagram.svg")),
    //           ],
    //       },
    //       …
    //   ];
    // -------------------------------------------------------------------------
    let skills_dir = Path::new(&manifest_dir).join("skills");
    let skill_entries = collect_entries_by_subdir(&skills_dir);

    let skills_out = Path::new(&out_dir).join("skills_embedded.rs");
    let mut out = fs::File::create(&skills_out).expect("Failed to create skills_embedded.rs");
    writeln!(out, "const SKILLS: &[SkillEntry] = &[").unwrap();
    for (skill_name, files) in &skill_entries {
        writeln!(out, "    SkillEntry {{").unwrap();
        writeln!(out, "        skill: {skill_name:?},").unwrap();
        writeln!(out, "        files: &[").unwrap();
        for (rel, abs) in files {
            writeln!(out, "            ({rel:?}, include_str!({abs:?})),").unwrap();
        }
        writeln!(out, "        ],").unwrap();
        writeln!(out, "    }},").unwrap();
    }
    writeln!(out, "];").unwrap();

    // -------------------------------------------------------------------------
    // plugins/  →  plugins_embedded.rs
    //
    // Plugins keep a flat per-file layout (one PluginFile entry per file).
    // Recursive collection is used here too so nested plugin assets work,
    // but the struct shape is unchanged.
    //
    //   const PLUGIN_FILES: &[PluginFile] = &[
    //       PluginFile { plugin: "arcgis-ge", file: "GeoEngineTools.pyt", content: include_str!("…") },
    //       …
    //   ];
    // -------------------------------------------------------------------------
    let plugins_dir = Path::new(&manifest_dir).join("plugins");
    let plugin_entries = collect_entries_by_subdir(&plugins_dir);

    let plugins_out = Path::new(&out_dir).join("plugins_embedded.rs");
    let mut out = fs::File::create(&plugins_out).expect("Failed to create plugins_embedded.rs");
    writeln!(out, "const PLUGIN_FILES: &[PluginFile] = &[").unwrap();
    for (plugin_name, files) in &plugin_entries {
        for (rel, abs) in files {
            writeln!(
                out,
                "    PluginFile {{ plugin: {plugin_name:?}, file: {rel:?}, content: include_str!({abs:?}) }},",
            ).unwrap();
        }
    }
    writeln!(out, "];").unwrap();
}

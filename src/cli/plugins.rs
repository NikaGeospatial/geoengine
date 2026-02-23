use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;
use regex::Regex;
use crate::config::worker::{WorkerConfig, InputParameter};

/// Install the GeoEngine plugin into ArcGIS Pro's toolbox directory.
pub async fn register_arcgis(custom_path: Option<PathBuf>) -> Result<()> {
    println!(
        "{} Registering GeoEngine with ArcGIS Pro...",
        "=>".blue().bold()
    );

    let toolbox_dir = if let Some(path) = custom_path {
        path
    } else {
        find_arcgis_toolbox_dir()?
    };

    std::fs::create_dir_all(&toolbox_dir)?;

    write_arcgis_plugin(&toolbox_dir)?;
    println!(
        "{} Installed GeoEngine toolbox to: {}",
        "✓".green().bold(),
        toolbox_dir.display()
    );

    Ok(())
}

/// Install the GeoEngine plugin into QGIS's plugin directory.
pub async fn register_qgis(custom_path: Option<PathBuf>) -> Result<()> {
    println!(
        "{} Registering GeoEngine with QGIS...",
        "=>".blue().bold()
    );

    let plugin_dir = if let Some(path) = custom_path {
        path
    } else {
        find_qgis_plugin_dir()?
    };

    let geoengine_dir = plugin_dir.join("geoengine");
    std::fs::create_dir_all(&geoengine_dir)?;

    write_qgis_plugin(&geoengine_dir)?;

    println!(
        "{} Installed GeoEngine plugin to: {}",
        "✓".green().bold(),
        geoengine_dir.display()
    );

    Ok(())
}

/// Debug helper that installs the QGIS plugin only when missing.
pub async fn debug_qgis() -> Result<()> {
    if verify_qgis_plugin_installed()? {
        println!(
            "{} QGIS plugin is already installed. No action taken.",
            "=>".yellow().bold()
        );
        return Ok(());
    }

    register_qgis(None).await
}

fn missing_files(base: &PathBuf, required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter_map(|f| {
            let p = base.join(f);
            if p.exists() { None } else { Some((*f).to_string()) }
        })
        .collect()
}

/// Check if the GeoEngine plugin is installed in the ArcGIS Pro toolbox directory.
pub fn verify_arcgis_plugin_installed() -> Result<bool> {
    let arcgis_dir = find_arcgis_toolbox_dir()?;
    let arcgis_required = ["GeoEngineTools.pyt", "geoengine_client.py"];
    let arcgis_missing = missing_files(&arcgis_dir, &arcgis_required);
    Ok(arcgis_missing.is_empty())
}

/// Check if the GeoEngine plugin is installed in the QGIS plugin directory.
pub fn verify_qgis_plugin_installed() -> Result<bool> {
    let qgis_dir = find_qgis_plugin_dir()?.join("geoengine");
    let qgis_required = ["__init__.py", "geoengine_plugin.py", "geoengine_provider.py", "metadata.txt"];
    let qgis_missing = missing_files(&qgis_dir, &qgis_required);
    Ok(qgis_missing.is_empty())
}

pub fn find_arcgis_toolbox_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    let candidates = [
        home.join("Documents").join("ArcGIS").join("Toolboxes"),
        home.join("ArcGIS").join("Toolboxes"),
    ];

    for candidate in &candidates {
        if candidate.parent().map(|p| p.exists()).unwrap_or(false) {
            return Ok(candidate.clone());
        }
    }

    Ok(candidates[0].clone())
}

fn find_qgis_plugin_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    #[cfg(target_os = "windows")]
    let plugin_dir = home
        .join("AppData")
        .join("Roaming")
        .join("QGIS")
        .join("QGIS3")
        .join("profiles")
        .join("default")
        .join("python")
        .join("plugins");

    #[cfg(target_os = "macos")]
    let plugin_dir = home
        .join("Library")
        .join("Application Support")
        .join("QGIS")
        .join("QGIS3")
        .join("profiles")
        .join("default")
        .join("python")
        .join("plugins");

    #[cfg(target_os = "linux")]
    let plugin_dir = home
        .join(".local")
        .join("share")
        .join("QGIS")
        .join("QGIS3")
        .join("profiles")
        .join("default")
        .join("python")
        .join("plugins");

    Ok(plugin_dir)
}

fn write_arcgis_plugin(dir: &PathBuf) -> Result<()> {
    let toolbox_content = include_str!("../../plugins/arcgis-ge/GeoEngineTools.pyt");
    std::fs::write(dir.join("GeoEngineTools.pyt"), toolbox_content)?;

    let client_content = include_str!("../../plugins/arcgis-ge/geoengine_client.py");
    std::fs::write(dir.join("geoengine_client.py"), client_content)?;

    Ok(())
}

fn write_qgis_plugin(dir: &PathBuf) -> Result<()> {
    let init_content = include_str!("../../plugins/qgis-ge/__init__.py");
    std::fs::write(dir.join("__init__.py"), init_content)?;

    let plugin_content = include_str!("../../plugins/qgis-ge/geoengine_plugin.py");
    std::fs::write(dir.join("geoengine_plugin.py"), plugin_content)?;

    let provider_content = include_str!("../../plugins/qgis-ge/geoengine_provider.py");
    std::fs::write(dir.join("geoengine_provider.py"), provider_content)?;

    let metadata_content = include_str!("../../plugins/qgis-ge/metadata.txt");
    std::fs::write(dir.join("metadata.txt"), metadata_content)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// GeoEngine.pyt static toolbox management
// ---------------------------------------------------------------------------

/// Convert a worker name like "my-worker" to a PascalCase Python class name like "MyWorker".
fn to_pascal_case(name: &str) -> String {
    name.split(|c: char| c == '-' || c == '_' || c == ' ')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut chars = s.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect()
}

/// Map a geoengine.yaml input type to the matching arcpy datatype string.
fn arcpy_datatype(param_type: &str) -> &'static str {
    match param_type {
        "file"     => "DEFile",
        "folder"   => "DEFolder",
        "number"   => "GPDouble",
        "boolean"  => "GPBoolean",
        "datetime" => "GPDate",
        _           => "GPString",  // string, enum, unknown
    }
}

/// Render one arcpy.Parameter block (4-space indented, assigned to variable `pN`).
fn render_parameter(idx: usize, inp: &InputParameter) -> String {
    // arcpy parameter names must use underscores, not dashes
    let arcpy_name = inp.name.replace('-', "_");
    let datatype   = arcpy_datatype(&inp.param_type);
    let required   = if inp.required.unwrap_or(true) { "Required" } else { "Optional" };
    let display    = inp.description.as_deref().unwrap_or(&inp.name);
    // folder/file with readonly: false is an output parameter in ArcGIS terms
    let direction = if (inp.param_type == "folder" || inp.param_type == "file")
        && inp.readonly == Some(false)
    {
        "Output"
    } else {
        "Input"
    };
    let var = format!("p{}", idx);

    let mut lines = format!(
        "        {var} = arcpy.Parameter(\n\
         \x20           displayName=\"{display}\",\n\
         \x20           name=\"{arcpy_name}\",\n\
         \x20           datatype=\"{datatype}\",\n\
         \x20           parameterType=\"{required}\",\n\
         \x20           direction=\"{direction}\",\n\
         \x20       )\n",
        var = var,
        display = display,
        arcpy_name = arcpy_name,
        datatype = datatype,
        required = required,
        direction = direction,
    );

    // Default value
    if let Some(default) = &inp.default {
        let default_str = match default {
            serde_yaml::Value::String(s) => format!("\"{}\"", s),
            serde_yaml::Value::Bool(b)   => if *b { "True".to_string() } else { "False".to_string() },
            serde_yaml::Value::Number(n) => n.to_string(),
            _                            => "None".to_string(),
        };
        lines.push_str(&format!("        {}.value = {}\n", var, default_str));
    }

    // Enum filter
    if inp.param_type == "enum" {
        if let Some(values) = &inp.enum_values {
            let list_str = values
                .iter()
                .map(|v| format!("\"{}\"", v))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push_str(&format!("        {}.filter.type = \"ValueList\"\n", var));
            lines.push_str(&format!("        {}.filter.list = [{}]\n", var, list_str));
        }
    }

    lines
}

/// Build the Python source block for a single tool class with real parameter info and execute.
fn build_tool_class(
    class_name: &str,
    label: &str,
    description: &str,
    worker_name: &str,
    inputs: &[InputParameter],
) -> String {
    // --- getParameterInfo body ---
    let get_param_body = if inputs.is_empty() {
        "        params = []\n        return params\n".to_string()
    } else {
        let mut body = String::new();
        let var_names: Vec<String> = (0..inputs.len()).map(|i| format!("p{}", i)).collect();

        for (i, inp) in inputs.iter().enumerate() {
            body.push_str(&render_parameter(i, inp));
        }
        body.push_str(&format!("        return [{}]\n", var_names.join(", ")));
        body
    };

    // --- execute body ---
    // Build the input collection loop; name used for --input must keep original dashes
    let execute_body = format!(
        concat!(
            "        import subprocess\n",
            "        cmd = [\"geoengine\", \"run\", \"{worker}\", \"--json\"]\n",
            "        for p in parameters:\n",
            "            if p.value is not None:\n",
            "                raw = p.value.dataSource if hasattr(p.value, 'dataSource') else str(p.value)\n",
            "                cmd += [\"--input\", f\"{{p.name.replace('_', '-')}}={{raw}}\"]\n",
            "        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)\n",
            "        for line in proc.stderr:\n",
            "            messages.addMessage(line.rstrip())\n",
            "        proc.wait()\n",
            "        if proc.returncode != 0:\n",
            "            raise Exception(f\"Worker '{worker}' failed with exit code {{proc.returncode}}\")\n",
        ),
        worker = worker_name,
    );

    format!(
        concat!(
            "\n",
            "class {class_name}:\n",
            "    def __init__(self):\n",
            "        self.label = \"{label}\"\n",
            "        self.description = \"{description}\"\n",
            "\n",
            "    def getParameterInfo(self):\n",
            "{get_param_body}",
            "\n",
            "    def isLicensed(self):\n",
            "        return True\n",
            "\n",
            "    def updateParameters(self, parameters):\n",
            "        return\n",
            "\n",
            "    def updateMessages(self, parameters):\n",
            "        return\n",
            "\n",
            "    def execute(self, parameters, messages):\n",
            "{execute_body}",
            "\n",
            "    def postExecute(self, parameters):\n",
            "        return\n",
        ),
        class_name = class_name,
        label = label,
        description = description,
        get_param_body = get_param_body,
        execute_body = execute_body,
    )
}

/// Build the Toolbox header with a given list of tool class names.
fn build_toolbox_header(tool_class_names: &[String]) -> String {
    let tools_list = if tool_class_names.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", tool_class_names.join(", "))
    };
    format!(
        concat!(
            "# -*- coding: utf-8 -*-\n",
            "import arcpy\n",
            "\n",
            "\n",
            "class Toolbox:\n",
            "    def __init__(self):\n",
            "        self.label = \"GeoEngine\"\n",
            "        self.alias = \"geoengine\"\n",
            "        self.tools = {tools_list}\n",
        ),
        tools_list = tools_list,
    )
}

/// Collect all tool class names in the file (every `class Foo:` that is not `Toolbox`).
fn collect_tool_class_names(content: &str) -> Vec<String> {
    let re = Regex::new(r"(?m)^class (\w+):").unwrap();
    re.captures_iter(content)
        .map(|c| c[1].to_string())
        .filter(|n| n != "Toolbox")
        .collect()
}

/// Strip the tool class block for `class_name` from the file content.
/// Uses a simple line-by-line scan so no lookahead regex is needed.
fn strip_tool_class(content: &str, class_name: &str) -> String {
    let target_header = format!("class {}:", class_name);
    let mut output: Vec<&str> = Vec::new();
    let mut skipping = false;

    for line in content.split('\n') {
        if skipping {
            // Stop skipping when we hit the next top-level class definition.
            if line.starts_with("class ") && !line.starts_with(&target_header) {
                skipping = false;
                output.push(line);
            }
            // Otherwise drop the line (it belongs to the target class block).
        } else if line.trim_end() == target_header || line.starts_with(&target_header) {
            skipping = true;
        } else {
            output.push(line);
        }
    }

    // Collapse runs of 3+ blank lines down to 2.
    let joined = output.join("\n");
    let blank_re = Regex::new(r"\n{3,}").unwrap();
    blank_re.replace_all(&joined, "\n\n").to_string()
}


/// Update the `self.tools = [...]` line inside the Toolbox class.
fn update_tools_list(content: &str, tool_class_names: &[String]) -> String {
    let tools_list = if tool_class_names.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", tool_class_names.join(", "))
    };
    let re = Regex::new(r"(?m)^        self\.tools = \[.*?\]").unwrap();
    re.replace(content, format!("        self.tools = {}", tools_list).as_str())
        .to_string()
}

/// Create or update `GeoEngine.pyt` in `dir` to include a static tool class for the given worker.
pub fn write_arcgis_worker_tool(
    dir: &PathBuf,
    worker_name: &str,
    config: &WorkerConfig,
) -> Result<()> {
    let pyt_path = dir.join("GeoEngine.pyt");
    let class_name = to_pascal_case(worker_name);
    let label: String = worker_name
        .split(|c: char| c == '-' || c == '_')
        .map(|s| {
            let mut chars = s.chars();
            match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let description = config.description.as_deref().unwrap_or("");
    let empty_inputs: Vec<InputParameter> = Vec::new();
    let inputs: &[InputParameter] = config
        .command
        .as_ref()
        .and_then(|c| c.inputs.as_deref())
        .unwrap_or(&empty_inputs);

    let existing = if pyt_path.exists() {
        std::fs::read_to_string(&pyt_path)
            .with_context(|| format!("Failed to read {}", pyt_path.display()))?
    } else {
        build_toolbox_header(&[])
    };

    // Remove old version of this class, then append the updated one.
    let stripped = strip_tool_class(&existing, &class_name);
    let new_block = build_tool_class(&class_name, &label, description, worker_name, inputs);
    let combined = format!("{}{}", stripped.trim_end(), new_block);

    // Rebuild self.tools list
    let names = collect_tool_class_names(&combined);
    let final_content = update_tools_list(&combined, &names);

    std::fs::create_dir_all(dir)?;
    std::fs::write(&pyt_path, final_content)
        .with_context(|| format!("Failed to write {}", pyt_path.display()))?;

    Ok(())
}

/// Remove the static tool class for `worker_name` from `GeoEngine.pyt`.
/// No-op if the file does not exist.
pub fn remove_arcgis_worker_tool(dir: &PathBuf, worker_name: &str) -> Result<()> {
    let pyt_path = dir.join("GeoEngine.pyt");
    if !pyt_path.exists() {
        return Ok(());
    }

    let class_name = to_pascal_case(worker_name);
    let content = std::fs::read_to_string(&pyt_path)
        .with_context(|| format!("Failed to read {}", pyt_path.display()))?;

    let stripped = strip_tool_class(&content, &class_name);
    let names = collect_tool_class_names(&stripped);
    let final_content = update_tools_list(&stripped, &names);

    std::fs::write(&pyt_path, final_content)
        .with_context(|| format!("Failed to write {}", pyt_path.display()))?;

    Ok(())
}

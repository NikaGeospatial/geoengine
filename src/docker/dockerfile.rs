use std::fs::File;
use std::io::{self, BufRead, Result};
use std::path::{Path, PathBuf};
use colored::Colorize;
use crate::config::worker::{WorkerConfig, CommandConfig};
use regex::Regex;

pub fn get_dockerfile_config(current_dir: &PathBuf, config: &mut WorkerConfig) -> Result<()> {
    let path = Path::new(&current_dir).join("Dockerfile");
    let file_res = File::open(path);

    let file = match file_res {
        Ok(file) => file,
        Err(_) => {
            println!("No Dockerfile found in current directory. Skipping discovery.");
            return Ok(());
        }
    };

    let reader = io::BufReader::new(file);
    let mut iter = reader.lines().peekable();
    let mut entry = false;

    println!("Found Dockerfile, discovering configurations...");

    while let Some(line_result) = iter.next() {
        let line = line_result?;
        let line_trimmed = line.trim();
        let (key, value) = map_dockerfile_line(line_trimmed, iter.peek().is_none(), entry)
            .unwrap_or_else(|| (String::new(), String::new()));
        match key.as_str() {
            "handler" => {
                entry = true;
                println!("{} discovered: {}", "command".bold(), value.cyan());
                // Split handler into program + script (e.g., "python main.py")
                let parts: Vec<&str> = value.split_whitespace().collect();
                if parts.len() >= 2 {
                    config.command = Some(CommandConfig {
                        program: parts[0].to_string(),
                        script: parts[1..].join(" "),
                        inputs: config.command.as_ref().and_then(|c| c.inputs.clone()),
                    });
                } else if parts.len() == 1 {
                    config.command = Some(CommandConfig {
                        program: parts[0].to_string(),
                        script: String::new(),
                        inputs: config.command.as_ref().and_then(|c| c.inputs.clone()),
                    });
                }
            },
            _ => {}
        }
    }
    println!();
    println!("Discovery complete and reflected in config file.");
    println!("{} {} {}", "Do".bold(), "not".red().bold(), "change the above values manually.".bold());
    println!("Run `geoengine apply` to apply changes from Dockerfile.");

    Ok(())
}

fn map_dockerfile_line(line: &str, last: bool, entry: bool) -> Option<(String, String)> {
    let words: Vec<&str> = line.trim().split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let head = words[0];
    let cmd = words[1..].to_vec();

    match head {
        "ENTRYPOINT" => {
            let entry = cmd.join(" ");
            let re = Regex::new(r#"[\[\]'",]"#).unwrap();
            let entry_clean = re.replace_all(entry.as_str(), "");
            Some(("handler".to_string(), entry_clean.to_string()))
        },
        "CMD" => {
            if last && !entry {
                let entry = cmd.join(" ");
                let re = Regex::new(r#"[\[\]'",]"#).unwrap();
                let entry_clean = re.replace_all(entry.as_str(), "");
                Some(("handler".to_string(), entry_clean.to_string()))
            }
            else {
                None
            }
        }
        _ => None
    }
}

use anyhow::Result;
use std::path::PathBuf;
use clap::Subcommand;
use colored::Colorize;
use regex::Regex;
use dotenvy::from_path_iter;
use crate::config::settings::Settings;

/// Custom parser that splits a string on the first `=`
fn parse_key_val(s: &str) -> Result<(String, String)> {
    // We use split_once to ensure we only split on the *first* '='.
    // This allows values to contain '=' (e.g., URL=https://example.com?q=1)
    match s.trim_end().split_once('=') {
        Some((key, value)) => {
            // Ensures that there are no trailing/leading whitespaces for key and value
            if key.trim() != key || value.trim() != value {
                return Err(anyhow::anyhow!("{}{}{}",
                    "Invalid format: ".to_string().yellow(),
                    s.yellow().italic(),
                    " should not have whitespaces around '='".to_string().yellow(),
                ));
            }

            // Ensures no whitespace in key
            if key.trim().contains(char::is_whitespace) {
                return Err(anyhow::anyhow!("{}{}{}",
                    "Invalid format: key ".to_string().yellow(),
                    key.yellow().italic(),
                    " contains whitespace.".to_string().yellow(),
                ));
            }
            // Ensures key is ascii
            if !key.trim().is_ascii() {
                return Err(anyhow::anyhow!("{}{}{}",
                    "Invalid format: key ".to_string().yellow(),
                    key.yellow().italic(),
                    " contains illegal characters.".to_string().yellow(),
                ));
            }

            let quote_checker = Regex::new(r#"^(".*"|'.*')$"#)?;
            // Ensures no whitespace in value if not contained in quotes
            if value.contains(char::is_whitespace) && !quote_checker.is_match(value) {
                return Err(anyhow::anyhow!("{}",
                    "Invalid format: values with whitespace must be contained in quotes".to_string().yellow()
                ));
            }

            // This strips outer double quotes and single quotes from the value
            let clean_value = value.trim_matches('"').trim_matches('\'');
            Ok((key.to_string(), clean_value.to_string()))
        },
        None => Err(anyhow::anyhow!("{}{}{}",
                    "Invalid format: ".to_string().yellow(),
                    s.yellow().italic(),
                    " must be in KEY=VALUE format.".to_string().yellow(),
                )),
    }
}

#[derive(Subcommand)]
pub enum EnvCommands {
    /// Set an environment variable
    Set {
        /// Environment variable key-value pair (format: KEY=VALUE)
        #[arg(value_parser = parse_key_val)]
        var: Vec<(String, String)>,

        /// Set from a file
        #[arg(short, long)]
        file: Option<String>,
    },

    /// Unset an environment variable
    Unset {
        /// Environment variable key
        key: Vec<String>,
    },

    /// List all environment variables
    List,

    /// Show the current value of an environment variable
    Show {
        /// Environment variable key
        key: String,
    },
}

impl EnvCommands {
    pub async fn execute(self) -> Result<()> {
        match self {
            EnvCommands::Set { var, file } => set_env_vars(var, file).await,
            EnvCommands::Unset { key } => unset_env_var(key).await,
            EnvCommands::List => list_env_vars().await,
            EnvCommands::Show { key } => show_env_var(key.as_ref()).await,
        }
    }
}

/// Set environment variables from vector of key-value pairs
/// If file is specified, read from file and set environment variables from it.
/// If both file and key-value pairs are specified, key-value pairs take precedence.
async fn set_env_vars(vars: Vec<(String, String)>, file: Option<String>) -> Result<()> {
    if vars.is_empty() && file.is_none() {
        anyhow::bail!("No variables provided. Pass KEY=VALUE and/or -f <path>.");
    }

    let mut settings = Settings::load()?;
    if file.is_some() {
        let path = PathBuf::from(file.as_deref().unwrap());
        let var_iter = from_path_iter(path)?;

        for item in var_iter {
            let (key, val) = item?;
            settings.set_env(key.as_ref(), val.as_ref())?
        }
    }
    for (key, value) in vars {
        settings.set_env(key.as_ref(), value.as_ref())?;
    }
    settings.save()?;
    Ok(())
}

/// Unset environment variables from vector of keys
async fn unset_env_var(keys: Vec<String>) -> Result<()> {
    if keys.is_empty() {
        anyhow::bail!("No variable names provided. Pass one or more keys.");
    }

    let mut settings = Settings::load()?;
    for key in keys {
        match settings.remove_env(key.as_ref()) {
            Ok(_) => {},
            Err(e) => println!("{}{}",
                e.to_string().yellow(),
                ", skipped.".to_string().yellow(),
            ),
        };
    }
    settings.save()?;
    Ok(())
}
}

async fn list_env_vars() -> Result<()> {
    let settings = Settings::load()?;
    let env = settings.list_env();
    match env {
        None => println!("No environment variables set."),
        Some(env) => {
            let key_m = env.keys().map(|k| k.chars().count()).max();
            let val_m = env.values().map(|v| v.chars().count()).max();
            println!();
            println!("{:<w1$} {:<w2$}",
                     "KEY".bold(),
                     "VALUE".bold(),
                     w1 = key_m.unwrap_or(3) + 1,
                     w2 = val_m.unwrap_or(5)
            );
            println!("{}-{}",
                     "-".repeat(key_m.unwrap_or(3) + 1),
                     "-".repeat(val_m.unwrap_or(5))
            );
            for (key, value) in env {
                println!("{:<w1$} {:<w2$}",
                    key,
                    value,
                    w1 = key_m.unwrap_or(3) + 1,
                    w2 = val_m.unwrap_or(5)
                );
            }
            println!();
        },
    }
    Ok(())
}

async fn show_env_var(key: &str) -> Result<()> {
    let settings = Settings::load()?;
    let env = settings.get_env(key);
    match env {
        None => println!("{}{}{}",
            "Environment variable ".yellow(),
            key.yellow().italic(),
            " not found.".yellow()
        ),
        Some(val) => println!("{}", val.bold()),
    }
    Ok(())
}
//! How the bind password is obtained. Never stored in the config file.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Deserializer};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PasswordSource {
    /// Prompt the operator interactively (no echo).
    #[default]
    Prompt,
    /// Read from the named environment variable.
    Env(String),
    /// Run a shell command; its stdout (trailing newline trimmed) is the password.
    Command(String),
}

impl FromStr for PasswordSource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        if s == "prompt" {
            Ok(PasswordSource::Prompt)
        } else if let Some(var) = s.strip_prefix("env:") {
            if var.is_empty() {
                return Err(anyhow!("password_source 'env:' needs a variable name"));
            }
            Ok(PasswordSource::Env(var.to_string()))
        } else if let Some(cmd) = s.strip_prefix("command:") {
            if cmd.trim().is_empty() {
                return Err(anyhow!("password_source 'command:' needs a command"));
            }
            Ok(PasswordSource::Command(cmd.to_string()))
        } else {
            Err(anyhow!(
                "invalid password_source '{s}': expected 'prompt', 'env:VAR', or 'command:...'"
            ))
        }
    }
}

impl<'de> Deserialize<'de> for PasswordSource {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl PasswordSource {
    pub fn resolve(&self) -> Result<String> {
        match self {
            PasswordSource::Prompt => rpassword::prompt_password("LDAP bind password: ")
                .context("reading password from prompt"),
            PasswordSource::Env(var) => std::env::var(var)
                .with_context(|| format!("environment variable '{var}' is not set")),
            PasswordSource::Command(cmd) => run_password_command(cmd),
        }
    }
}

fn run_password_command(cmd: &str) -> Result<String> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("running password command '{cmd}'"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "password command '{cmd}' exited with status {}",
            output.status
        ));
    }
    let pw =
        String::from_utf8(output.stdout).context("password command output was not valid UTF-8")?;
    Ok(pw.trim_end_matches(['\n', '\r']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_variant() {
        assert_eq!(
            "prompt".parse::<PasswordSource>().unwrap(),
            PasswordSource::Prompt
        );
        assert_eq!(
            "env:EDAPTOR_PW".parse::<PasswordSource>().unwrap(),
            PasswordSource::Env("EDAPTOR_PW".to_string())
        );
        assert_eq!(
            "command:pass ldap/mgr".parse::<PasswordSource>().unwrap(),
            PasswordSource::Command("pass ldap/mgr".to_string())
        );
    }

    #[test]
    fn rejects_unknown_and_empty() {
        assert!("nonsense".parse::<PasswordSource>().is_err());
        assert!("env:".parse::<PasswordSource>().is_err());
        assert!("command:   ".parse::<PasswordSource>().is_err());
    }

    #[test]
    fn resolves_from_env() {
        std::env::set_var("EDAPTOR_TEST_PW_VAR", "s3cret");
        let src = PasswordSource::Env("EDAPTOR_TEST_PW_VAR".to_string());
        assert_eq!(src.resolve().unwrap(), "s3cret");
    }

    #[test]
    fn resolves_from_command_and_trims_newline() {
        let src = PasswordSource::Command("printf 'hunter2\\n'".to_string());
        assert_eq!(src.resolve().unwrap(), "hunter2");
    }

    #[test]
    fn failing_command_errors() {
        let src = PasswordSource::Command("exit 3".to_string());
        assert!(src.resolve().is_err());
    }
}

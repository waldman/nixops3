use anyhow::Result;
use std::process::Command;
use crate::traits::Executor;

pub struct ProcessExecutor;

impl Executor for ProcessExecutor {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<(i32, String)> {
        let output = Command::new(cmd)
            .args(args)
            .output()?;
        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let combined = format!("{}{}", stdout, stderr);
        Ok((code, combined))
    }
}

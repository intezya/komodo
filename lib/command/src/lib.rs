use std::{
  path::{Path, PathBuf},
  process::Stdio,
  sync::OnceLock,
  time::Duration,
};

use komodo_client::{
  entities::{komodo_timestamp, update::Log},
  parsers::parse_multiline_command,
};

mod output;

pub use output::*;
use tokio::process::Command;

/// Commands are run directly, and cannot include '&&'
pub async fn run_komodo_standard_command(
  stage: &str,
  path: impl Into<Option<&Path>>,
  command: impl Into<String>,
) -> Log {
  let command = command.into();
  let start_ts = komodo_timestamp();
  let output = run_standard_command(&command, path).await;
  output_into_log(stage, command, start_ts, output)
}

pub async fn run_komodo_standard_command_with_timeout(
  stage: &str,
  path: impl Into<Option<&Path>>,
  command: impl Into<String>,
  timeout: Duration,
) -> Log {
  let command = command.into();
  let start_ts = komodo_timestamp();
  let output =
    run_standard_command_with_timeout(&command, path, timeout).await;
  output_into_log(stage, command, start_ts, output)
}

/// Commands are wrapped in 'sh -c', and can include '&&'
pub async fn run_komodo_shell_command(
  stage: &str,
  path: impl Into<Option<&Path>>,
  command: impl Into<String>,
) -> Log {
  let command = command.into();
  let start_ts = komodo_timestamp();
  let output = run_shell_command(&command, path).await;
  output_into_log(stage, command, start_ts, output)
}

pub async fn run_komodo_shell_command_with_timeout(
  stage: &str,
  path: impl Into<Option<&Path>>,
  command: impl Into<String>,
  timeout: Duration,
) -> Log {
  let command = command.into();
  let start_ts = komodo_timestamp();
  let output =
    run_shell_command_with_timeout(&command, path, timeout).await;
  output_into_log(stage, command, start_ts, output)
}

/// Parses commands out of multiline string
/// and chains them together with '&&'.
/// Supports full line and end of line comments.
/// See [parse_multiline_command].
///
/// The result may be None if the command is empty after parsing,
/// ie if all the lines are commented out.
pub async fn run_komodo_multiline_command(
  stage: &str,
  path: impl Into<Option<&Path>>,
  command: impl AsRef<str>,
) -> Option<Log> {
  let command = parse_multiline_command(command);
  if command.is_empty() {
    return None;
  }
  Some(run_komodo_shell_command(stage, path, command).await)
}

pub enum KomodoCommandMode {
  Standard,
  Shell,
  Multiline,
}

/// Executes the command, and sanitizes the output to avoid exposing secrets in the log.
///
/// Checks to make sure the command is non-empty after being multiline-parsed.
///
/// If `parse_multiline: true`, parses commands out of multiline string
/// and chains them together with '&&'.
/// Supports full line and end of line comments.
/// See [parse_multiline_command].
pub async fn run_komodo_command_with_sanitization(
  stage: &str,
  path: impl Into<Option<&Path>>,
  command: impl AsRef<str>,
  mode: KomodoCommandMode,
  replacers: &[(String, String)],
) -> Option<Log> {
  let mut log = match mode {
    KomodoCommandMode::Standard => run_komodo_standard_command(
      stage,
      path,
      command.as_ref().to_string(),
    )
    .await
    .into(),
    KomodoCommandMode::Shell => run_komodo_shell_command(
      stage,
      path,
      command.as_ref().to_string(),
    )
    .await
    .into(),
    KomodoCommandMode::Multiline => {
      run_komodo_multiline_command(stage, path, command).await
    }
  }?;

  // Sanitize the command and output
  log.command = svi::replace_in_string(&log.command, replacers);
  log.stdout = svi::replace_in_string(&log.stdout, replacers);
  log.stderr = svi::replace_in_string(&log.stderr, replacers);

  Some(log)
}

pub fn output_into_log(
  stage: &str,
  command: String,
  start_ts: i64,
  output: CommandOutput,
) -> Log {
  let success = output.success();
  Log {
    stage: stage.to_string(),
    stdout: output.stdout,
    stderr: output.stderr,
    command,
    success,
    start_ts,
    end_ts: komodo_timestamp(),
  }
}

/// Commands are run directly, and cannot include '&&'
pub async fn run_standard_command(
  command: &str,
  path: impl Into<Option<&Path>>,
) -> CommandOutput {
  run_standard_command_inner(command, path, None).await
}

pub async fn run_standard_command_with_timeout(
  command: &str,
  path: impl Into<Option<&Path>>,
  timeout: Duration,
) -> CommandOutput {
  run_standard_command_inner(command, path, Some(timeout)).await
}

async fn run_standard_command_inner(
  command: &str,
  path: impl Into<Option<&Path>>,
  timeout: Option<Duration>,
) -> CommandOutput {
  let lexed = if let Some(lexed) = shlex::split(command)
    && !lexed.is_empty()
  {
    lexed
  } else {
    return CommandOutput::from_err(std::io::Error::other(
      "Command lexed into empty args",
    ));
  };

  let mut cmd = Command::new(&lexed[0]);

  cmd
    .args(&lexed[1..])
    .kill_on_drop(true)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

  if let Some(path) = path.into() {
    match path.canonicalize() {
      Ok(path) => {
        cmd.current_dir(path);
      }
      Err(e) => return CommandOutput::from_err(e),
    }
  }

  run_command_output(cmd, timeout).await
}

fn shell() -> &'static str {
  static DEFAULT_SHELL: OnceLock<String> = OnceLock::new();
  DEFAULT_SHELL.get_or_init(|| {
    if PathBuf::from("/bin/bash").exists() {
      String::from("/bin/bash")
    } else if PathBuf::from("/usr/bin/bash").exists() {
      String::from("/usr/bin/bash")
    } else if PathBuf::from("/bin/sh").exists() {
      String::from("/bin/sh")
    } else if PathBuf::from("/usr/bin/sh").exists() {
      String::from("/usr/bin/sh")
    } else {
      // try to use sh wherever it is on host by name.
      String::from("sh")
    }
  })
}

/// Commands are wrapped in 'sh -c', and can include '&&'
pub async fn run_shell_command(
  command: &str,
  path: impl Into<Option<&Path>>,
) -> CommandOutput {
  run_shell_command_inner(command, path, None).await
}

pub async fn run_shell_command_with_timeout(
  command: &str,
  path: impl Into<Option<&Path>>,
  timeout: Duration,
) -> CommandOutput {
  run_shell_command_inner(command, path, Some(timeout)).await
}

async fn run_shell_command_inner(
  command: &str,
  path: impl Into<Option<&Path>>,
  timeout: Option<Duration>,
) -> CommandOutput {
  let mut cmd = Command::new(shell());

  cmd
    .args(["-c", command])
    .kill_on_drop(true)
    .stdin(Stdio::null());

  if let Some(path) = path.into() {
    match path.canonicalize() {
      Ok(path) => {
        cmd.current_dir(path);
      }
      Err(e) => return CommandOutput::from_err(e),
    }
  }

  run_command_output(cmd, timeout).await
}

async fn run_command_output(
  mut cmd: Command,
  timeout: Option<Duration>,
) -> CommandOutput {
  match timeout {
    Some(timeout) => {
      match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(output) => CommandOutput::from(output),
        Err(_) => CommandOutput::from_timeout(timeout),
      }
    }
    None => CommandOutput::from(cmd.output().await),
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::{
    run_shell_command_with_timeout, run_standard_command_with_timeout,
  };

  #[tokio::test]
  async fn standard_command_timeout_returns_failure() {
    let out = run_standard_command_with_timeout(
      "sleep 2",
      None,
      Duration::from_millis(100),
    )
    .await;

    assert!(!out.success());
    assert!(out.stderr.contains("Command timed out"));
  }

  #[tokio::test]
  async fn standard_command_before_timeout_returns_success() {
    let out = run_standard_command_with_timeout(
      "printf ok",
      None,
      Duration::from_secs(5),
    )
    .await;

    assert!(out.success());
    assert_eq!(out.stdout, "ok");
  }

  #[tokio::test]
  async fn shell_command_timeout_returns_failure() {
    let out = run_shell_command_with_timeout(
      "sleep 2",
      None,
      Duration::from_millis(100),
    )
    .await;

    assert!(!out.success());
    assert!(out.stderr.contains("Command timed out"));
  }

  #[tokio::test]
  async fn shell_command_before_timeout_returns_success() {
    let out = run_shell_command_with_timeout(
      "printf ok",
      None,
      Duration::from_secs(5),
    )
    .await;

    assert!(out.success());
    assert_eq!(out.stdout, "ok");
  }
}

use std::{
  io,
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
use tokio::{
  io::{AsyncBufReadExt, AsyncRead, BufReader},
  process::Command,
  sync::mpsc,
};

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

pub async fn run_komodo_standard_command_with_merged_output_with_timeout(
  stage: &str,
  path: impl Into<Option<&Path>>,
  command: impl Into<String>,
  timeout: Duration,
) -> (Log, String) {
  let command = command.into();
  let start_ts = komodo_timestamp();
  let MergedCommandOutput {
    output,
    merged_output,
  } = run_standard_command_with_merged_output_with_timeout(
    &command, path, timeout,
  )
  .await;
  (
    output_into_log(stage, command, start_ts, output),
    merged_output,
  )
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

#[derive(Debug, Clone)]
pub struct MergedCommandOutput {
  pub output: CommandOutput,
  pub merged_output: String,
}

impl MergedCommandOutput {
  fn from_err(error: io::Error) -> Self {
    Self {
      output: CommandOutput::from_err(error),
      merged_output: String::new(),
    }
  }

  fn from_timeout(timeout: Duration) -> Self {
    Self {
      output: CommandOutput::from_timeout(timeout),
      merged_output: String::new(),
    }
  }
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

pub async fn run_standard_command_with_merged_output_with_timeout(
  command: &str,
  path: impl Into<Option<&Path>>,
  timeout: Duration,
) -> MergedCommandOutput {
  run_standard_command_with_merged_output_inner(
    command,
    path,
    Some(timeout),
  )
  .await
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

async fn run_standard_command_with_merged_output_inner(
  command: &str,
  path: impl Into<Option<&Path>>,
  timeout: Option<Duration>,
) -> MergedCommandOutput {
  let lexed = if let Some(lexed) = shlex::split(command)
    && !lexed.is_empty()
  {
    lexed
  } else {
    return MergedCommandOutput::from_err(std::io::Error::other(
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
      Err(error) => return MergedCommandOutput::from_err(error),
    }
  }

  run_command_output_with_merged_stream(cmd, timeout).await
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

async fn run_command_output_with_merged_stream(
  cmd: Command,
  timeout: Option<Duration>,
) -> MergedCommandOutput {
  match timeout {
    Some(timeout) => {
      match tokio::time::timeout(
        timeout,
        collect_command_output_with_merged_stream(cmd),
      )
      .await
      {
        Ok(output) => output,
        Err(_) => MergedCommandOutput::from_timeout(timeout),
      }
    }
    None => collect_command_output_with_merged_stream(cmd).await,
  }
}

async fn collect_command_output_with_merged_stream(
  mut cmd: Command,
) -> MergedCommandOutput {
  let mut child = match cmd.spawn() {
    Ok(child) => child,
    Err(error) => return MergedCommandOutput::from_err(error),
  };

  let stdout = if let Some(stdout) = child.stdout.take() {
    stdout
  } else {
    return MergedCommandOutput::from_err(std::io::Error::other(
      "Failed to capture stdout",
    ));
  };
  let stderr = if let Some(stderr) = child.stderr.take() {
    stderr
  } else {
    return MergedCommandOutput::from_err(std::io::Error::other(
      "Failed to capture stderr",
    ));
  };

  let (tx, mut rx) = mpsc::unbounded_channel();
  let stdout_task = tokio::spawn(read_stream(stdout, tx.clone()));
  let stderr_task = tokio::spawn(read_stream(stderr, tx.clone()));
  drop(tx);

  let mut merged_output = String::new();
  while let Some(chunk) = rx.recv().await {
    merged_output.push_str(&chunk);
  }

  let stdout = match stdout_task.await {
    Ok(Ok(stdout)) => stdout,
    Ok(Err(error)) => return MergedCommandOutput::from_err(error),
    Err(error) => {
      return MergedCommandOutput::from_err(std::io::Error::other(
        format!("Failed to join stdout reader: {error}"),
      ));
    }
  };
  let stderr = match stderr_task.await {
    Ok(Ok(stderr)) => stderr,
    Ok(Err(error)) => return MergedCommandOutput::from_err(error),
    Err(error) => {
      return MergedCommandOutput::from_err(std::io::Error::other(
        format!("Failed to join stderr reader: {error}"),
      ));
    }
  };
  let status = match child.wait().await {
    Ok(status) => status,
    Err(error) => return MergedCommandOutput::from_err(error),
  };

  MergedCommandOutput {
    output: CommandOutput {
      status,
      stdout,
      stderr,
    },
    merged_output,
  }
}

async fn read_stream<R>(
  reader: R,
  sender: mpsc::UnboundedSender<String>,
) -> io::Result<String>
where
  R: AsyncRead + Unpin + Send + 'static,
{
  let mut reader = BufReader::new(reader);
  let mut output = String::new();

  loop {
    let mut line = String::new();
    let read = reader.read_line(&mut line).await?;
    if read == 0 {
      break;
    }
    output.push_str(&line);
    let _ = sender.send(line);
  }

  Ok(output)
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::{
    run_shell_command_with_timeout,
    run_standard_command_with_merged_output_with_timeout,
    run_standard_command_with_timeout,
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
  async fn merged_output_preserves_cross_stream_order() {
    let out = run_standard_command_with_merged_output_with_timeout(
      "sh -lc 'echo stdout-1; sleep 0.05; echo stderr-1 >&2; sleep 0.05; echo stdout-2; sleep 0.05; echo stderr-2 >&2'",
      None,
      Duration::from_secs(5),
    )
    .await;

    assert!(out.output.success());
    assert_eq!(out.output.stdout, "stdout-1\nstdout-2\n");
    assert_eq!(out.output.stderr, "stderr-1\nstderr-2\n");
    assert_eq!(
      out.merged_output,
      "stdout-1\nstderr-1\nstdout-2\nstderr-2\n"
    );
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

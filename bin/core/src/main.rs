#![allow(unused_crate_dependencies)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let mut term_signal = tokio::signal::unix::signal(
    tokio::signal::unix::SignalKind::terminate(),
  )?;
  tokio::select! {
    res = tokio::spawn(komodo_core::app()) => res?,
    _ = term_signal.recv() => Ok(()),
  }
}

#[cfg(test)]
mod deploy_stack_tests {
  use komodo_client::entities::update::Log;

  fn log(stage: &str, success: bool) -> Log {
    Log {
      stage: stage.into(),
      success,
      ..Default::default()
    }
  }

  #[test]
  fn deploy_update_success_allows_failed_pull_when_up_succeeded() {
    assert!(crate::api::execute::deploy_update_success(
      true,
      &[log("Compose Pull", false), log("Compose Up", true)]
    ));
  }

  #[test]
  fn deploy_update_success_rejects_failed_up() {
    assert!(!crate::api::execute::deploy_update_success(
      false,
      &[log("Compose Pull", false), log("Compose Up", false)]
    ));
  }

  #[test]
  fn deploy_update_success_rejects_other_failed_stages() {
    assert!(!crate::api::execute::deploy_update_success(
      true,
      &[log("Compose Up", true), log("Refresh Stack Info", false),]
    ));
  }
}

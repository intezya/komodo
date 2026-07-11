use std::{
  borrow::Cow, collections::HashSet, path::Path, time::Duration,
};

use anyhow::{Context, anyhow};
use command::{
  KomodoCommandMode, run_komodo_command_with_sanitization,
};
use komodo_client::entities::{
  docker::container::{ContainerStateStatusEnum, HealthStatusEnum},
  update::Log,
};
use shell_escape::unix::escape;

use crate::state::docker_client;

const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(60);
const RUNNING_STABILITY: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub(super) struct RolloutServiceArgs<'a> {
  pub run_directory: &'a Path,
  pub start_command: String,
  pub scale_command: String,
  pub project_name: &'a str,
  pub service_name: &'a str,
  pub desired_replicas: usize,
  pub pre_stop_hook: Option<&'a str>,
  pub replacers: &'a [(String, String)],
}

pub(super) async fn rollout_service(
  args: RolloutServiceArgs<'_>,
  logs: &mut Vec<Log>,
) -> anyhow::Result<()> {
  let original_ids =
    service_container_ids(args.project_name, args.service_name)
      .await?;
  if original_ids.is_empty() {
    let Some(start_log) = run_komodo_command_with_sanitization(
      "Rolling Start Service",
      args.run_directory,
      args.start_command,
      KomodoCommandMode::Shell,
      args.replacers,
    )
    .await
    else {
      unreachable!()
    };
    let start_succeeded = start_log.success;
    logs.push(start_log);
    return start_succeeded.then_some(()).ok_or_else(|| {
      anyhow!("failed to start service '{}'", args.service_name)
    });
  }
  if original_ids.len() != args.desired_replicas {
    return Err(anyhow!(
      "service '{}' has {} containers but declares {} replicas",
      args.service_name,
      original_ids.len(),
      args.desired_replicas
    ));
  }

  for old_id in original_ids {
    let before =
      service_container_ids(args.project_name, args.service_name)
        .await?;
    let Some(scale_log) = run_komodo_command_with_sanitization(
      "Rolling Scale Up",
      args.run_directory,
      args.scale_command.clone(),
      KomodoCommandMode::Shell,
      args.replacers,
    )
    .await
    else {
      unreachable!()
    };
    let scale_succeeded = scale_log.success;
    logs.push(scale_log);
    if !scale_succeeded {
      return Err(anyhow!(
        "failed to create replacement for service '{}'",
        args.service_name
      ));
    }

    let after =
      service_container_ids(args.project_name, args.service_name)
        .await?;
    let new_id = exactly_one_new_container(&before, &after)?;

    if let Err(error) = wait_until_ready(&new_id).await {
      cleanup_new_container(
        &new_id,
        args.run_directory,
        args.replacers,
        logs,
      )
      .await;
      return Err(error.context(format!(
        "replacement container {new_id} for service '{}' did not become ready",
        args.service_name
      )));
    }

    if let Some(hook) = args.pre_stop_hook {
      let command = format!(
        "docker exec {} sh -c {}",
        escape(Cow::Borrowed(&old_id)),
        escape(Cow::Borrowed(hook))
      );
      let Some(hook_log) = run_komodo_command_with_sanitization(
        "Rolling Pre Stop Hook",
        args.run_directory,
        command,
        KomodoCommandMode::Shell,
        args.replacers,
      )
      .await
      else {
        unreachable!()
      };
      let hook_succeeded = hook_log.success;
      logs.push(hook_log);
      if !hook_succeeded {
        cleanup_new_container(
          &new_id,
          args.run_directory,
          args.replacers,
          logs,
        )
        .await;
        return Err(anyhow!(
          "pre-stop hook failed for container {old_id}"
        ));
      }
    }

    let remove_command = format!(
      "docker stop {} && docker rm {}",
      escape(Cow::Borrowed(&old_id)),
      escape(Cow::Borrowed(&old_id))
    );
    let Some(remove_log) = run_komodo_command_with_sanitization(
      "Rolling Remove Old Container",
      args.run_directory,
      remove_command,
      KomodoCommandMode::Shell,
      args.replacers,
    )
    .await
    else {
      unreachable!()
    };
    let remove_succeeded = remove_log.success;
    logs.push(remove_log);
    if !remove_succeeded {
      return Err(anyhow!("failed to remove old container {old_id}"));
    }
  }

  let final_ids =
    service_container_ids(args.project_name, args.service_name)
      .await?;
  if final_ids.len() != args.desired_replicas {
    return Err(anyhow!(
      "service '{}' finished with {} containers instead of {}",
      args.service_name,
      final_ids.len(),
      args.desired_replicas
    ));
  }
  Ok(())
}

async fn service_container_ids(
  project_name: &str,
  service_name: &str,
) -> anyhow::Result<Vec<String>> {
  let client = docker_client().load();
  let client = client
    .iter()
    .next()
    .context("could not connect to docker client")?;
  let mut ids = client
    .list_containers()
    .await?
    .into_iter()
    .filter(|container| {
      container
        .labels
        .get("com.docker.compose.project")
        .zip(container.labels.get("com.docker.compose.service"))
        .is_some_and(|(project, service)| {
          project == project_name && service == service_name
        })
    })
    .filter_map(|container| container.id)
    .collect::<Vec<_>>();
  ids.sort();
  Ok(ids)
}

async fn wait_until_ready(container_id: &str) -> anyhow::Result<()> {
  let client = docker_client().load();
  let client = client
    .iter()
    .next()
    .context("could not connect to docker client")?;
  let first = client.inspect_container(container_id).await?;
  let has_healthcheck = first
    .state
    .as_ref()
    .and_then(|state| state.health.as_ref())
    .is_some();
  let deadline = tokio::time::Instant::now()
    + if has_healthcheck {
      HEALTHCHECK_TIMEOUT
    } else {
      RUNNING_STABILITY
    };

  loop {
    let container = client.inspect_container(container_id).await?;
    let state = container
      .state
      .context("container inspect response has no state")?;
    if state.status != ContainerStateStatusEnum::Running {
      return Err(anyhow!(
        "container entered state '{}'",
        state.status
      ));
    }
    if let Some(health) = state.health {
      match health.status {
        HealthStatusEnum::Healthy => return Ok(()),
        HealthStatusEnum::Unhealthy => {
          return Err(anyhow!("container became unhealthy"));
        }
        _ => {}
      }
    } else if tokio::time::Instant::now() >= deadline {
      return Ok(());
    }
    if tokio::time::Instant::now() >= deadline {
      return Err(anyhow!("container readiness timed out"));
    }
    tokio::time::sleep(POLL_INTERVAL).await;
  }
}

async fn cleanup_new_container(
  container_id: &str,
  run_directory: &Path,
  replacers: &[(String, String)],
  logs: &mut Vec<Log>,
) {
  let command = format!(
    "docker stop {} && docker rm {}",
    escape(Cow::Borrowed(container_id)),
    escape(Cow::Borrowed(container_id))
  );
  if let Some(log) = run_komodo_command_with_sanitization(
    "Rolling Cleanup New Container",
    run_directory,
    command,
    KomodoCommandMode::Shell,
    replacers,
  )
  .await
  {
    logs.push(log);
  }
}

fn exactly_one_new_container(
  before: &[String],
  after: &[String],
) -> anyhow::Result<String> {
  let before = before.iter().collect::<HashSet<_>>();
  let new = after
    .iter()
    .filter(|id| !before.contains(id))
    .collect::<Vec<_>>();
  match new.as_slice() {
    [id] => Ok((*id).clone()),
    _ => Err(anyhow!(
      "expected exactly one new container, found {}",
      new.len()
    )),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use command::run_komodo_shell_command;

  #[test]
  fn detects_exactly_one_new_container() {
    let before = vec!["old-a".into(), "old-b".into()];
    let after = vec!["old-a".into(), "old-b".into(), "new-c".into()];

    assert_eq!(
      exactly_one_new_container(&before, &after)
        .expect("one new container should be accepted"),
      "new-c"
    );
  }

  #[test]
  fn rejects_ambiguous_new_container_set() {
    let before = vec!["old-a".into()];
    let after = vec!["old-a".into(), "new-b".into(), "new-c".into()];

    let error = exactly_one_new_container(&before, &after)
      .expect_err("multiple new containers must be rejected");

    assert!(error.to_string().contains("found 2"));
  }

  #[tokio::test]
  async fn replaces_two_live_replicas_with_one_container_surge() {
    if std::env::var_os("KOMODO_RUN_DOCKER_ROLLOUT_TEST").is_none() {
      return;
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
      .parent()
      .and_then(Path::parent)
      .expect("periphery crate should be under repository root");
    let file = repo.join("compose/rollout.test.compose.yaml");
    let project = "komodo-rollout-test";
    let base = format!(
      "docker compose -p {project} -f {}",
      escape(Cow::Owned(file.display().to_string()))
    );
    let _ = run_komodo_shell_command(
      "Test Cleanup",
      repo,
      format!("{base} down --remove-orphans"),
    )
    .await;
    let up = run_komodo_shell_command(
      "Test Setup",
      repo,
      format!("{base} up -d --scale web=2"),
    )
    .await;
    assert!(up.success, "{}", up.stderr);
    let before = service_container_ids(project, "web")
      .await
      .expect("test containers should be listed");
    let mut logs = Vec::new();

    let result = rollout_service(
      RolloutServiceArgs {
        run_directory: repo,
        start_command: format!("{base} up -d --scale web=2 web"),
        scale_command: format!(
          "{base} up -d --no-deps --no-recreate --scale web=3 web"
        ),
        project_name: project,
        service_name: "web",
        desired_replicas: 2,
        pre_stop_hook: None,
        replacers: &[],
      },
      &mut logs,
    )
    .await;
    let after = service_container_ids(project, "web").await;
    let cleanup = run_komodo_shell_command(
      "Test Cleanup",
      repo,
      format!("{base} down --remove-orphans"),
    )
    .await;

    assert!(cleanup.success, "{}", cleanup.stderr);
    result.expect("rollout should succeed");
    let after = after.expect("test containers should be listed");
    assert_eq!(after.len(), 2);
    assert!(before.iter().all(|id| !after.contains(id)));
  }

  #[tokio::test]
  async fn removes_unhealthy_replacement_and_preserves_old_replicas()
  {
    if std::env::var_os("KOMODO_RUN_DOCKER_ROLLOUT_TEST").is_none() {
      return;
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
      .parent()
      .and_then(Path::parent)
      .expect("periphery crate should be under repository root");
    let file = repo.join("compose/rollout.test.compose.yaml");
    let unhealthy =
      repo.join("compose/rollout.unhealthy.test.compose.yaml");
    let project = "komodo-rollout-unhealthy-test";
    let base = format!(
      "docker compose -p {project} -f {}",
      escape(Cow::Owned(file.display().to_string()))
    );
    let unhealthy_base = format!(
      "{base} -f {}",
      escape(Cow::Owned(unhealthy.display().to_string()))
    );
    let _ = run_komodo_shell_command(
      "Test Cleanup",
      repo,
      format!("{base} down --remove-orphans"),
    )
    .await;
    let up = run_komodo_shell_command(
      "Test Setup",
      repo,
      format!("{base} up -d --scale web=2"),
    )
    .await;
    assert!(up.success, "{}", up.stderr);
    let before = service_container_ids(project, "web")
      .await
      .expect("test containers should be listed");
    let mut logs = Vec::new();

    let result = rollout_service(
      RolloutServiceArgs {
        run_directory: repo,
        start_command: format!("{base} up -d --scale web=2 web"),
        scale_command: format!(
          "{unhealthy_base} up -d --no-deps --no-recreate --scale web=3 web"
        ),
        project_name: project,
        service_name: "web",
        desired_replicas: 2,
        pre_stop_hook: None,
        replacers: &[],
      },
      &mut logs,
    )
    .await;
    let after = service_container_ids(project, "web").await;
    let cleanup = run_komodo_shell_command(
      "Test Cleanup",
      repo,
      format!("{base} down --remove-orphans"),
    )
    .await;

    assert!(cleanup.success, "{}", cleanup.stderr);
    assert!(result.is_err());
    let after = after.expect("test containers should be listed");
    assert_eq!(after, before);
    assert!(
      logs
        .iter()
        .any(|log| log.stage == "Rolling Cleanup New Container")
    );
  }
}

use anyhow::anyhow;
use interpolate::Interpolator;
use komodo_client::{
  api::execute::*,
  entities::{
    SwarmOrServer,
    permission::PermissionLevel,
    repo::Repo,
    server::Server,
    stack::{Stack, StackActionState},
    update::{Log, Update},
    user::User,
  },
};
use periphery_client::api::compose::*;

use crate::{
  helpers::{
    periphery_client,
    query::{VariablesAndSecrets, get_variables_and_secrets},
    stack_git_token,
    update::update_update,
  },
  monitor::refresh_server_cache,
  periphery::PeripheryClient,
  resource,
  state::action_states,
};

use super::setup_stack_execution;

pub trait ExecuteCompose {
  type Extras;

  async fn execute(
    periphery: PeripheryClient,
    stack: Stack,
    services: Vec<String>,
    extras: Self::Extras,
  ) -> anyhow::Result<Vec<Log>>;
}

pub async fn execute_compose<T: ExecuteCompose>(
  stack: &str,
  services: Vec<String>,
  user: &User,
  set_in_progress: impl Fn(&mut StackActionState),
  update: Update,
  extras: T::Extras,
) -> anyhow::Result<Update> {
  let (stack, swarm_or_server) = setup_stack_execution(
    stack,
    user,
    PermissionLevel::Execute.into(),
  )
  .await?;

  let SwarmOrServer::Server(server) = swarm_or_server else {
    return Err(anyhow!(
      "Compose executions (Start, Stop, Restart) should not be called for Stack in Swarm Mode"
    ));
  };

  execute_compose_with_stack_and_server::<T>(
    stack,
    server,
    services,
    set_in_progress,
    update,
    extras,
  )
  .await
}

pub async fn execute_compose_with_stack_and_server<
  T: ExecuteCompose,
>(
  stack: Stack,
  server: Server,
  services: Vec<String>,
  set_in_progress: impl Fn(&mut StackActionState),
  mut update: Update,
  extras: T::Extras,
) -> anyhow::Result<Update> {
  // get the action state for the stack (or insert default).
  let action_state =
    action_states().stack.get_or_insert_default(&stack.id).await;

  // Will check to ensure stack not already busy before updating, and return Err if so.
  // The returned guard will set the action state back to default when dropped.
  let _action_guard = action_state.update(set_in_progress)?;

  // Send update here for UI to recheck action state
  update_update(update.clone()).await?;

  let periphery = periphery_client(&server).await?;

  if !services.is_empty() {
    update.logs.push(Log::simple(
      "Service/s",
      format!(
        "Execution requested for Stack service/s {}",
        services.join(", ")
      ),
    ))
  }

  update
    .logs
    .extend(T::execute(periphery, stack, services, extras).await?);

  // Ensure cached stack state up to date by updating server cache
  refresh_server_cache(&server, true).await;

  update.finalize();
  update_update(update.clone()).await?;

  Ok(update)
}

fn service_args(services: &[String]) -> String {
  if !services.is_empty() {
    format!(" {}", services.join(" "))
  } else {
    String::new()
  }
}

impl ExecuteCompose for StartStack {
  type Extras = ();
  async fn execute(
    periphery: PeripheryClient,
    stack: Stack,
    services: Vec<String>,
    _: Self::Extras,
  ) -> anyhow::Result<Vec<Log>> {
    let service_args = service_args(&services);
    let log = periphery
      .request(ComposeExecution {
        project: stack.project_name(false),
        command: format!("start{service_args}"),
      })
      .await?;
    Ok(vec![log])
  }
}

impl ExecuteCompose for RestartStack {
  type Extras = ();
  async fn execute(
    periphery: PeripheryClient,
    stack: Stack,
    services: Vec<String>,
    _: Self::Extras,
  ) -> anyhow::Result<Vec<Log>> {
    let mut logs = Vec::new();
    let mut stack = stack;
    let mut repo = if !stack.config.files_on_host
      && !stack.config.linked_repo.is_empty()
    {
      Some(
        resource::get::<Repo>(&stack.config.linked_repo)
          .await?
          .into(),
      )
    } else {
      None
    };

    let git_token =
      stack_git_token(&mut stack, repo.as_mut()).await?;

    let secret_replacers = if !stack.config.skip_secret_interp {
      let VariablesAndSecrets { variables, secrets } =
        get_variables_and_secrets().await?;

      let mut interpolator =
        Interpolator::new(Some(&variables), &secrets);

      interpolator.interpolate_stack(&mut stack)?;
      if let Some(repo) = repo.as_mut()
        && !repo.config.skip_secret_interp
      {
        interpolator.interpolate_repo(repo)?;
      }
      interpolator.push_logs(&mut logs);

      interpolator.secret_replacers
    } else {
      Default::default()
    };

    let res = periphery
      .request(ComposeForceRecreate {
        stack,
        services,
        repo,
        git_token,
        replacers: secret_replacers.into_iter().collect(),
      })
      .await?;

    logs.extend(res.logs);
    Ok(logs)
  }
}

impl ExecuteCompose for PauseStack {
  type Extras = ();
  async fn execute(
    periphery: PeripheryClient,
    stack: Stack,
    services: Vec<String>,
    _: Self::Extras,
  ) -> anyhow::Result<Vec<Log>> {
    let service_args = service_args(&services);
    let log = periphery
      .request(ComposeExecution {
        project: stack.project_name(false),
        command: format!("pause{service_args}"),
      })
      .await?;
    Ok(vec![log])
  }
}

impl ExecuteCompose for UnpauseStack {
  type Extras = ();
  async fn execute(
    periphery: PeripheryClient,
    stack: Stack,
    services: Vec<String>,
    _: Self::Extras,
  ) -> anyhow::Result<Vec<Log>> {
    let service_args = service_args(&services);
    let log = periphery
      .request(ComposeExecution {
        project: stack.project_name(false),
        command: format!("unpause{service_args}"),
      })
      .await?;
    Ok(vec![log])
  }
}

impl ExecuteCompose for StopStack {
  type Extras = Option<i32>;
  async fn execute(
    periphery: PeripheryClient,
    stack: Stack,
    services: Vec<String>,
    timeout: Self::Extras,
  ) -> anyhow::Result<Vec<Log>> {
    let service_args = service_args(&services);
    let maybe_timeout = maybe_timeout(timeout);
    let log = periphery
      .request(ComposeExecution {
        project: stack.project_name(false),
        command: format!("stop{maybe_timeout}{service_args}"),
      })
      .await?;
    Ok(vec![log])
  }
}

impl ExecuteCompose for DestroyStack {
  type Extras = (Option<i32>, bool);
  async fn execute(
    periphery: PeripheryClient,
    stack: Stack,
    services: Vec<String>,
    (timeout, remove_orphans): Self::Extras,
  ) -> anyhow::Result<Vec<Log>> {
    let service_args = service_args(&services);
    let maybe_timeout = maybe_timeout(timeout);
    let maybe_remove_orphans = if remove_orphans {
      " --remove-orphans"
    } else {
      ""
    };
    let log = periphery
      .request(ComposeExecution {
        project: stack.project_name(false),
        command: format!(
          "down{maybe_timeout}{maybe_remove_orphans}{service_args}"
        ),
      })
      .await?;
    Ok(vec![log])
  }
}

pub fn maybe_timeout(timeout: Option<i32>) -> String {
  if let Some(timeout) = timeout {
    format!(" --timeout {timeout}")
  } else {
    String::new()
  }
}

use std::collections::HashSet;

use anyhow::{Context, anyhow};
use komodo_client::{
  api::read::*,
  entities::{
    SwarmOrServer,
    docker::{
      container::Container, service::SwarmService, stack::SwarmStack,
    },
    permission::PermissionLevel,
    stack::{Stack, StackActionState, StackListItem, StackState},
  },
};
use mogh_error::AddStatusCodeError as _;
use mogh_resolver::Resolve;
use periphery_client::api::{
  compose::{GetComposeLog, GetComposeLogSearch},
  container::InspectContainer,
};
use reqwest::StatusCode;

use crate::{
  helpers::{
    periphery_client,
    query::{
      VariablesAndSecrets, get_all_tags, get_variables_and_secrets,
      redact_stack_display_secrets, stack_display_secret_replacers,
    },
    swarm::swarm_request,
  },
  permission::{
    get_check_permissions, get_user_permission_on_resource,
  },
  resource,
  stack::setup_stack_execution,
  state::{action_states, stack_status_cache},
};

use super::ReadArgs;

fn redact_stack_read_response(
  stack: &mut Stack,
  display_secret_replacers: &[(String, String)],
  redact_remote_contents: bool,
) {
  if let Some(deployed_config) = stack.info.deployed_config.as_mut() {
    *deployed_config = redact_stack_display_secrets(
      deployed_config,
      display_secret_replacers,
    );
  }

  if let Some(deployed_contents) =
    stack.info.deployed_contents.as_mut()
  {
    for deployed_contents in deployed_contents {
      deployed_contents.contents = redact_stack_display_secrets(
        &deployed_contents.contents,
        display_secret_replacers,
      );
    }
  }

  if redact_remote_contents
    && let Some(remote_contents) = stack.info.remote_contents.as_mut()
  {
    for remote_contents in remote_contents {
      remote_contents.contents = redact_stack_display_secrets(
        &remote_contents.contents,
        display_secret_replacers,
      );
    }
  }
}

fn redact_env_vars(
  env: &mut [String],
  display_secret_replacers: &[(String, String)],
) {
  for entry in env {
    *entry =
      redact_stack_display_secrets(entry, display_secret_replacers);
  }
}

fn redact_stack_container_inspect_response(
  container: &mut Container,
  display_secret_replacers: &[(String, String)],
) {
  if let Some(config) = container.config.as_mut() {
    redact_env_vars(&mut config.env, display_secret_replacers);
  }
}

fn redact_stack_swarm_service_inspect_response(
  service: &mut SwarmService,
  display_secret_replacers: &[(String, String)],
) {
  let Some(env) = service
    .spec
    .as_mut()
    .and_then(|spec| spec.task_template.as_mut())
    .and_then(|task_template| task_template.container_spec.as_mut())
    .and_then(|container_spec| container_spec.env.as_mut())
  else {
    return;
  };
  redact_env_vars(env, display_secret_replacers);
}

impl Resolve<ReadArgs> for GetStack {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<Stack> {
    let mut stack = get_check_permissions::<Stack>(
      &self.stack,
      user,
      PermissionLevel::Read.into(),
    )
    .await?;

    let VariablesAndSecrets { secrets, .. } =
      get_variables_and_secrets().await?;
    let display_secret_replacers =
      stack_display_secret_replacers(&secrets);

    let permission =
      get_user_permission_on_resource::<Stack>(user, &stack.id)
        .await?;
    // Keep writable file views raw so editors do not round-trip
    // redacted placeholders back onto disk.
    redact_stack_read_response(
      &mut stack,
      &display_secret_replacers,
      permission.level < PermissionLevel::Write,
    );

    Ok(stack)
  }
}

impl Resolve<ReadArgs> for ListStackServices {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<ListStackServicesResponse> {
    let stack = get_check_permissions::<Stack>(
      &self.stack,
      user,
      PermissionLevel::Read.into(),
    )
    .await?;

    let services = stack_status_cache()
      .get(&stack.id)
      .await
      .unwrap_or_default()
      .curr
      .services
      .clone();

    Ok(services)
  }
}

impl Resolve<ReadArgs> for GetStackLog {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<GetStackLogResponse> {
    let GetStackLog {
      stack,
      mut services,
      tail,
      timestamps,
    } = self;
    let (stack, swarm_or_server) = setup_stack_execution(
      &stack,
      user,
      PermissionLevel::Read.logs(),
    )
    .await?;

    swarm_or_server.verify_has_target()?;

    let log = match swarm_or_server {
      SwarmOrServer::None => unreachable!(),
      SwarmOrServer::Swarm(swarm) => {
        let service = services.pop().context(
          "Must pass single service for Swarm mode Stack logs",
        )?;
        swarm_request(
          &swarm.config.server_ids,
          periphery_client::api::swarm::GetSwarmServiceLog {
            // The actual service name on swarm will be stackname_servicename
            service: format!(
              "{}_{service}",
              stack.project_name(false)
            ),
            tail,
            timestamps,
            no_task_ids: false,
            no_resolve: false,
            details: false,
          },
        )
        .await
        .context("Failed to get stack service log from swarm")?
      }
      SwarmOrServer::Server(server) => periphery_client(&server)
        .await?
        .request(GetComposeLog {
          project: stack.project_name(false),
          services,
          tail,
          timestamps,
        })
        .await
        .context("Failed to get stack log from periphery")?,
    };

    Ok(log)
  }
}

impl Resolve<ReadArgs> for SearchStackLog {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<SearchStackLogResponse> {
    let SearchStackLog {
      stack,
      mut services,
      terms,
      combinator,
      invert,
      timestamps,
    } = self;
    let (stack, swarm_or_server) = setup_stack_execution(
      &stack,
      user,
      PermissionLevel::Read.logs(),
    )
    .await?;

    swarm_or_server.verify_has_target()?;

    let log = match swarm_or_server {
      SwarmOrServer::None => unreachable!(),
      SwarmOrServer::Swarm(swarm) => {
        let service = services.pop().context(
          "Must pass single service for Swarm mode Stack logs",
        )?;
        swarm_request(
          &swarm.config.server_ids,
          periphery_client::api::swarm::GetSwarmServiceLogSearch {
            service,
            terms,
            combinator,
            invert,
            timestamps,
            no_task_ids: false,
            no_resolve: false,
            details: false,
          },
        )
        .await
        .context("Failed to get stack service log from swarm")?
      }
      SwarmOrServer::Server(server) => periphery_client(&server)
        .await?
        .request(GetComposeLogSearch {
          project: stack.project_name(false),
          services,
          terms,
          combinator,
          invert,
          timestamps,
        })
        .await
        .context("Failed to search stack log from periphery")?,
    };

    Ok(log)
  }
}

impl Resolve<ReadArgs> for InspectStackContainer {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<Container> {
    let InspectStackContainer { stack, service } = self;
    let (stack, swarm_or_server) = setup_stack_execution(
      &stack,
      user,
      PermissionLevel::Read.inspect(),
    )
    .await?;

    let SwarmOrServer::Server(server) = swarm_or_server else {
      return Err(
        anyhow!(
          "InspectStackContainer should not be called for Stack in Swarm Mode"
        )
        .status_code(StatusCode::BAD_REQUEST),
      );
    };

    let services = &stack_status_cache()
      .get(&stack.id)
      .await
      .unwrap_or_default()
      .curr
      .services;

    let Some(name) = services
      .iter()
      .find(|s| s.service == service)
      .and_then(|s| s.container.as_ref().map(|c| c.name.clone()))
    else {
      return Err(anyhow!(
        "No service found matching '{service}'. Was the stack last deployed manually?"
      ).into());
    };

    let mut res = periphery_client(&server)
      .await?
      .request(InspectContainer { name })
      .await
      .context("Failed to inspect container on server")?;

    let VariablesAndSecrets { secrets, .. } =
      get_variables_and_secrets().await?;
    let display_secret_replacers =
      stack_display_secret_replacers(&secrets);
    redact_stack_container_inspect_response(
      &mut res,
      &display_secret_replacers,
    );

    Ok(res)
  }
}

impl Resolve<ReadArgs> for InspectStackSwarmService {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<SwarmService> {
    let InspectStackSwarmService { stack, service } = self;
    let (stack, swarm_or_server) = setup_stack_execution(
      &stack,
      user,
      PermissionLevel::Read.inspect(),
    )
    .await?;

    let SwarmOrServer::Swarm(swarm) = swarm_or_server else {
      return Err(
        anyhow!(
          "InspectStackSwarmService should only be called for Stack in Swarm Mode"
        )
        .status_code(StatusCode::BAD_REQUEST),
      );
    };

    let services = &stack_status_cache()
      .get(&stack.id)
      .await
      .unwrap_or_default()
      .curr
      .services;

    let Some(service) = services
      .iter()
      .find(|s| s.service == service)
      .and_then(|s| {
        s.swarm_service.as_ref().and_then(|c| c.name.clone())
      })
    else {
      return Err(anyhow!(
        "No service found matching '{service}'. Was the stack last deployed manually?"
      ).into());
    };

    let mut res = swarm_request(
      &swarm.config.server_ids,
      periphery_client::api::swarm::InspectSwarmService { service },
    )
    .await
    .context("Failed to inspect service on swarm")?;

    let VariablesAndSecrets { secrets, .. } =
      get_variables_and_secrets().await?;
    let display_secret_replacers =
      stack_display_secret_replacers(&secrets);
    redact_stack_swarm_service_inspect_response(
      &mut res,
      &display_secret_replacers,
    );

    Ok(res)
  }
}

impl Resolve<ReadArgs> for InspectStackSwarmInfo {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<SwarmStack> {
    let (stack, swarm_or_server) = setup_stack_execution(
      &self.stack,
      user,
      PermissionLevel::Read.inspect(),
    )
    .await?;

    let SwarmOrServer::Swarm(swarm) = swarm_or_server else {
      return Err(
        anyhow!(
          "InspectStackSwarmInfo should only be called for Stack in Swarm Mode"
        )
        .status_code(StatusCode::BAD_REQUEST),
      );
    };

    swarm_request(
      &swarm.config.server_ids,
      periphery_client::api::swarm::InspectSwarmStack {
        stack: stack.project_name(false),
      },
    )
    .await
    .context("Failed to inspect stack info on swarm")
    .map_err(Into::into)
  }
}

impl Resolve<ReadArgs> for ListCommonStackExtraArgs {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<ListCommonStackExtraArgsResponse> {
    let all_tags = if self.query.tags.is_empty() {
      vec![]
    } else {
      get_all_tags(None).await?
    };
    let stacks = resource::list_full_for_user::<Stack>(
      self.query,
      user,
      PermissionLevel::Read.into(),
      &all_tags,
    )
    .await
    .context("Failed to get resources matching query")?;

    // first collect with guaranteed uniqueness
    let mut res = HashSet::<String>::new();

    for stack in stacks {
      for extra_arg in stack.config.extra_args {
        res.insert(extra_arg);
      }
    }

    let mut res = res.into_iter().collect::<Vec<_>>();
    res.sort();
    Ok(res)
  }
}

impl Resolve<ReadArgs> for ListCommonStackBuildExtraArgs {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<ListCommonStackBuildExtraArgsResponse> {
    let all_tags = if self.query.tags.is_empty() {
      vec![]
    } else {
      get_all_tags(None).await?
    };
    let stacks = resource::list_full_for_user::<Stack>(
      self.query,
      user,
      PermissionLevel::Read.into(),
      &all_tags,
    )
    .await
    .context("Failed to get resources matching query")?;

    // first collect with guaranteed uniqueness
    let mut res = HashSet::<String>::new();

    for stack in stacks {
      for extra_arg in stack.config.build_extra_args {
        res.insert(extra_arg);
      }
    }

    let mut res = res.into_iter().collect::<Vec<_>>();
    res.sort();
    Ok(res)
  }
}

impl Resolve<ReadArgs> for ListStacks {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<Vec<StackListItem>> {
    let all_tags = if self.query.tags.is_empty() {
      vec![]
    } else {
      get_all_tags(None).await?
    };
    let only_update_available = self.query.specific.update_available;
    let stacks = resource::list_for_user::<Stack>(
      self.query,
      user,
      PermissionLevel::Read.into(),
      &all_tags,
    )
    .await?;
    let stacks = if only_update_available {
      stacks
        .into_iter()
        .filter(|stack| {
          stack
            .info
            .services
            .iter()
            .any(|service| service.update_available)
        })
        .collect()
    } else {
      stacks
    };
    Ok(stacks)
  }
}

impl Resolve<ReadArgs> for ListFullStacks {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<ListFullStacksResponse> {
    let all_tags = if self.query.tags.is_empty() {
      vec![]
    } else {
      get_all_tags(None).await?
    };
    let mut stacks = resource::list_full_for_user::<Stack>(
      self.query,
      user,
      PermissionLevel::Read.into(),
      &all_tags,
    )
    .await?;

    let VariablesAndSecrets { secrets, .. } =
      get_variables_and_secrets().await?;
    let display_secret_replacers =
      stack_display_secret_replacers(&secrets);

    for stack in &mut stacks {
      redact_stack_read_response(
        stack,
        &display_secret_replacers,
        true,
      );
    }

    Ok(stacks)
  }
}

impl Resolve<ReadArgs> for GetStackActionState {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<StackActionState> {
    let stack = get_check_permissions::<Stack>(
      &self.stack,
      user,
      PermissionLevel::Read.into(),
    )
    .await?;
    let action_state = action_states()
      .stack
      .get(&stack.id)
      .await
      .unwrap_or_default()
      .get()?;
    Ok(action_state)
  }
}

impl Resolve<ReadArgs> for GetStacksSummary {
  async fn resolve(
    self,
    ReadArgs { user }: &ReadArgs,
  ) -> mogh_error::Result<GetStacksSummaryResponse> {
    let stacks = resource::list_full_for_user::<Stack>(
      Default::default(),
      user,
      PermissionLevel::Read.into(),
      &[],
    )
    .await
    .context("Failed to get stacks from database")?;

    let mut res = GetStacksSummaryResponse::default();

    let cache = stack_status_cache();

    for stack in stacks {
      res.total += 1;
      match cache.get(&stack.id).await.unwrap_or_default().curr.state
      {
        StackState::Running => res.running += 1,
        StackState::Stopped | StackState::Paused => res.stopped += 1,
        StackState::Down => res.down += 1,
        StackState::Unknown => {
          if !stack.template {
            res.unknown += 1
          }
        }
        _ => res.unhealthy += 1,
      }
    }

    Ok(res)
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use komodo_client::entities::{
    FileContents,
    docker::{
      ContainerConfig,
      service::ServiceSpec,
      task::{TaskSpec, TaskSpecContainerSpec},
    },
    stack::StackRemoteFileContents,
  };

  use super::*;

  fn sample_stack(secret: &str) -> Stack {
    let mut stack = Stack::default();
    stack.info.deployed_config =
      Some(format!("PASSWORD={secret}\nPUBLIC=value"));
    stack.info.remote_contents =
      Some(vec![StackRemoteFileContents {
        path: "compose.yaml".to_string(),
        contents: format!("PASSWORD={secret}\nPUBLIC=value"),
        ..Default::default()
      }]);
    stack
  }

  #[test]
  fn redacts_full_stack_read_payloads_for_read_only_views() {
    let mut stack = sample_stack("abc$def");

    redact_stack_read_response(
      &mut stack,
      &[("abc$def".to_string(), "[[SECRET]]".to_string())],
      true,
    );

    let deployed_config = stack.info.deployed_config.unwrap();
    assert!(!deployed_config.contains("abc$def"));
    assert!(deployed_config.contains("[[SECRET]]"));

    let remote_contents =
      stack.info.remote_contents.unwrap().pop().unwrap().contents;
    assert!(!remote_contents.contains("abc$def"));
    assert!(remote_contents.contains("[[SECRET]]"));
  }

  #[test]
  fn keeps_remote_contents_raw_for_writable_stack_views() {
    let mut stack = sample_stack("abc$def");

    redact_stack_read_response(
      &mut stack,
      &[("abc$def".to_string(), "[[SECRET]]".to_string())],
      false,
    );

    let deployed_config = stack.info.deployed_config.unwrap();
    assert!(!deployed_config.contains("abc$def"));
    assert!(deployed_config.contains("[[SECRET]]"));

    let remote_contents =
      stack.info.remote_contents.unwrap().pop().unwrap().contents;
    assert!(remote_contents.contains("abc$def"));
    assert!(!remote_contents.contains("[[SECRET]]"));
  }

  #[test]
  fn redacts_container_inspect_config_env_for_stack_secret() {
    let secret = "abc$def;$(whoami)'";
    let mut container = Container {
      config: Some(ContainerConfig {
        env: vec![
          format!("PASSWORD={secret}"),
          format!("COMBINED=prefix-{secret}-suffix"),
          "PUBLIC=value".to_string(),
        ],
        ..Default::default()
      }),
      ..Default::default()
    };

    redact_stack_container_inspect_response(
      &mut container,
      &[(secret.to_string(), "[[PASSWORD]]".to_string())],
    );

    let env = &container.config.as_ref().unwrap().env;
    assert!(!env.iter().any(|entry| entry.contains(secret)));
    assert!(env.contains(&"PASSWORD=<[[PASSWORD]]>".to_string()));
    assert!(env.contains(
      &"COMBINED=prefix-<[[PASSWORD]]>-suffix".to_string()
    ));
    assert!(env.contains(&"PUBLIC=value".to_string()));
  }

  #[test]
  fn redacts_swarm_service_inspect_env_for_stack_secret() {
    let secret = "abc$def;$(whoami)'";
    let mut service = SwarmService {
      spec: Some(ServiceSpec {
        task_template: Some(TaskSpec {
          container_spec: Some(TaskSpecContainerSpec {
            env: Some(vec![
              format!("PASSWORD={secret}"),
              format!("COMBINED=prefix-{secret}-suffix"),
              "PUBLIC=value".to_string(),
            ]),
            ..Default::default()
          }),
          ..Default::default()
        }),
        ..Default::default()
      }),
      ..Default::default()
    };

    redact_stack_swarm_service_inspect_response(
      &mut service,
      &[(secret.to_string(), "[[PASSWORD]]".to_string())],
    );

    let env = service
      .spec
      .as_ref()
      .unwrap()
      .task_template
      .as_ref()
      .unwrap()
      .container_spec
      .as_ref()
      .unwrap()
      .env
      .as_ref()
      .unwrap();
    assert!(!env.iter().any(|entry| entry.contains(secret)));
    assert!(env.contains(&"PASSWORD=<[[PASSWORD]]>".to_string()));
    assert!(env.contains(
      &"COMBINED=prefix-<[[PASSWORD]]>-suffix".to_string()
    ));
    assert!(env.contains(&"PUBLIC=value".to_string()));
  }

  #[test]
  fn redacts_interpolated_stack_secret_from_read_payload() {
    let secret = "abc$def;$(whoami)'";
    let mut secrets = HashMap::new();
    secrets.insert("PASSWORD".to_string(), secret.to_string());

    let file_contents = "services:\n  app:\n    environment:\n      - PASSWORD=[[PASSWORD]]\n";
    let mut emitted_stack = Stack::default();
    emitted_stack.config.file_contents = file_contents.to_string();
    interpolate::Interpolator::new(None, &secrets)
      .interpolate_stack(&mut emitted_stack)
      .unwrap();

    let mut stack = Stack::default();
    stack.config.file_contents = file_contents.to_string();
    stack.info.deployed_contents = Some(vec![FileContents {
      path: "compose.yaml".to_string(),
      contents: emitted_stack.config.file_contents.clone(),
    }]);
    stack.info.deployed_config =
      Some(emitted_stack.config.file_contents.clone());
    stack.info.remote_contents =
      Some(vec![StackRemoteFileContents {
        path: "compose.yaml".to_string(),
        contents: emitted_stack.config.file_contents.clone(),
        ..Default::default()
      }]);

    let stored_payload = serde_json::to_string(&stack).unwrap();
    assert!(stored_payload.contains(secret));

    let replacers = stack_display_secret_replacers(&secrets);
    redact_stack_read_response(&mut stack, &replacers, true);

    let payload = serde_json::to_string(&stack).unwrap();
    assert!(!payload.contains(secret));
    assert!(payload.contains("[[PASSWORD]]"));
  }
}

//! Safe Resource Sync execution from an already fetched Git snapshot.

use std::collections::{HashMap, HashSet};

use crate::{
  api::execute::ExecuteRequest,
  helpers::{
    all_resources::AllResourcesById,
    query::get_id_to_tags,
    update::{init_execution_update, update_update},
  },
  state::{action_states, db_client},
  sync::{
    ResourceSyncTrait, SyncExecutionMode,
    deploy::{
      SyncDeployParams, SyncDeployPolicy,
      build_deploy_cache_with_policy, deploy_from_cache,
    },
    execute::{ExecuteResourceSync, get_updates_for_execution},
    remote::RemoteResources,
  },
};
use anyhow::{Context, anyhow};
use database::mungos::{
  by_id::update_one_by_id, find::find_collect, mongodb::bson::doc,
};
use formatting::{Color, colored};
use komodo_client::{
  api::execute::RunSync,
  entities::{
    ResourceTarget,
    action::Action,
    alerter::Alerter,
    build::Build,
    builder::Builder,
    deployment::Deployment,
    komodo_timestamp,
    procedure::Procedure,
    repo::Repo,
    server::Server,
    stack::{PartialStackConfig, Stack, StackConfig},
    swarm::Swarm,
    sync::{DiffData, ResourceSync},
    toml::ResourceToml,
    update::Update,
    user::sync_user,
  },
};

fn gitops_delete_allowed() -> bool {
  false
}

/// Applies safe Create and Update changes from `remote` without fetching Git.
///
/// The caller has already resolved the effective Git source and owns the shared
/// source snapshot for this cycle. Delete deltas are calculated first, then
/// discarded only for execution so they remain available to pending-state code.
pub(super) async fn reconcile_prepared_sync(
  sync: ResourceSync,
  remote: RemoteResources,
) -> anyhow::Result<Update> {
  let request = ExecuteRequest::RunSync(RunSync {
    sync: sync.id.clone(),
    resource_type: None,
    resources: None,
  });
  let user = sync_user().clone();
  let mut update = init_execution_update(&request, &user).await?;

  let action_state =
    action_states().sync.get_or_insert_default(&sync.id).await;
  let _action_guard =
    action_state.update(|state| state.syncing = true)?;
  update_update(update.clone()).await?;

  let RemoteResources {
    resources,
    logs,
    hash,
    message,
    file_errors,
    ..
  } = remote;
  update.logs.extend(logs);
  update_update(update.clone()).await?;
  if !file_errors.is_empty() {
    return Err(anyhow!(
      "found file errors; cannot execute GitOps sync"
    ));
  }
  let mut resources = resources?;

  let pending_stack_deletes = pending_stack_delete_targets().await?;
  resources.stacks.retain(|stack| {
    !pending_stack_deletes.contains(&stack_target(stack))
  });

  let id_to_tags = get_id_to_tags(None).await?;
  let all_resources = AllResourcesById::load().await?;
  let deployments_by_name = all_resources
    .deployments
    .values()
    .filter(|deployment| {
      Deployment::include_resource(
        &deployment.name,
        &deployment.config,
        None,
        None,
        &deployment.tags,
        &id_to_tags,
        &sync.config.match_tags,
      )
    })
    .map(|deployment| (deployment.name.clone(), deployment.clone()))
    .collect::<HashMap<_, _>>();
  let stacks_by_name = all_resources
    .stacks
    .values()
    .filter(|stack| {
      Stack::include_resource(
        &stack.name,
        &stack.config,
        None,
        None,
        &stack.tags,
        &id_to_tags,
        &sync.config.match_tags,
      )
    })
    .map(|stack| (stack.name.clone(), stack.clone()))
    .collect::<HashMap<_, _>>();
  let deploy_cache = build_deploy_cache_with_policy(
    SyncDeployParams {
      deployments: &resources.deployments,
      deployment_map: &deployments_by_name,
      stacks: &resources.stacks,
      stack_map: &stacks_by_name,
    },
    SyncDeployPolicy::GitOps,
  )
  .await?;

  let delete = sync.config.managed || sync.config.delete;
  let execution_mode = if gitops_delete_allowed() {
    SyncExecutionMode::Manual
  } else {
    SyncExecutionMode::GitOpsSafe
  };
  macro_rules! get_deltas {
    ($(($var:ident, $Type:ident, $field:ident)),* $(,)?) => {
      $(
        let mut $var = if sync.config.include_resources {
          get_updates_for_execution::<$Type>(
            resources.$field,
            delete,
            None,
            None,
            &id_to_tags,
            &sync.config.match_tags,
          )
          .await?
        } else {
          Default::default()
        };
        $var.apply_execution_mode(execution_mode);
      )*
    };
  }
  get_deltas!(
    (server_deltas, Server, servers),
    (swarm_deltas, Swarm, swarms),
    (stack_deltas, Stack, stacks),
    (deployment_deltas, Deployment, deployments),
    (build_deltas, Build, builds),
    (repo_deltas, Repo, repos),
    (procedure_deltas, Procedure, procedures),
    (action_deltas, Action, actions),
    (builder_deltas, Builder, builders),
    (alerter_deltas, Alerter, alerters),
    (resource_sync_deltas, ResourceSync, resource_syncs),
  );

  let (
    variables_to_create,
    variables_to_update,
    mut variables_to_delete,
  ) = if sync.config.include_variables {
    crate::sync::variables::get_updates_for_execution(
      resources.variables,
      delete,
    )
    .await?
  } else {
    Default::default()
  };
  let (
    user_groups_to_create,
    user_groups_to_update,
    mut user_groups_to_delete,
  ) = if sync.config.include_user_groups {
    crate::sync::user_groups::get_updates_for_execution(
      resources.user_groups,
      delete,
    )
    .await?
  } else {
    Default::default()
  };
  if !gitops_delete_allowed() {
    variables_to_delete.clear();
    user_groups_to_delete.clear();
  }

  if deploy_cache.is_empty()
    && resource_sync_deltas.no_changes()
    && server_deltas.no_changes()
    && swarm_deltas.no_changes()
    && deployment_deltas.no_changes()
    && stack_deltas.no_changes()
    && build_deltas.no_changes()
    && builder_deltas.no_changes()
    && alerter_deltas.no_changes()
    && repo_deltas.no_changes()
    && procedure_deltas.no_changes()
    && action_deltas.no_changes()
    && user_groups_to_create.is_empty()
    && user_groups_to_update.is_empty()
    && variables_to_create.is_empty()
    && variables_to_update.is_empty()
  {
    update.push_simple_log(
      "No Changes",
      format!("{}. exiting.", colored("nothing to do", Color::Green)),
    );
  } else {
    maybe_extend(
      &mut update.logs,
      crate::sync::variables::run_updates(
        variables_to_create,
        variables_to_update,
        variables_to_delete,
      )
      .await,
    );
    maybe_extend(
      &mut update.logs,
      crate::sync::user_groups::run_updates(
        user_groups_to_create,
        user_groups_to_update,
        user_groups_to_delete,
      )
      .await,
    );
    maybe_extend(
      &mut update.logs,
      Server::execute_sync_updates(server_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Alerter::execute_sync_updates(alerter_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Action::execute_sync_updates(action_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Swarm::execute_sync_updates(swarm_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Builder::execute_sync_updates(builder_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Repo::execute_sync_updates(repo_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Build::execute_sync_updates(build_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Stack::execute_sync_updates(stack_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      ResourceSync::execute_sync_updates(resource_sync_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Deployment::execute_sync_updates(deployment_deltas).await,
    );
    maybe_extend(
      &mut update.logs,
      Procedure::execute_sync_updates(procedure_deltas).await,
    );
    deploy_from_cache(deploy_cache, &mut update.logs).await;
  }

  update_one_by_id(
    &db_client().resource_syncs,
    &sync.id,
    doc! {
      "$set": {
        "info.last_sync_ts": komodo_timestamp(),
        "info.last_sync_hash": hash,
        "info.last_sync_message": message,
      }
    },
    None,
  )
  .await
  .context("failed to update GitOps sync metadata")?;

  update.finalize();
  update_update(update.clone()).await?;
  Ok(update)
}

fn stack_target(
  stack: &ResourceToml<PartialStackConfig>,
) -> (String, String) {
  let config: StackConfig = stack.config.clone().into();
  let project_name = if config.project_name.is_empty() {
    stack.name.clone()
  } else {
    config.project_name
  };
  (config.server_id, project_name)
}

async fn pending_stack_delete_targets()
-> anyhow::Result<HashSet<(String, String)>> {
  let syncs =
    find_collect(&db_client().resource_syncs, None, None).await?;
  let mut targets = HashSet::new();
  for sync in syncs {
    for diff in sync.info.resource_updates {
      let (ResourceTarget::Stack(_), DiffData::Delete { current }) =
        (diff.target, diff.data)
      else {
        continue;
      };
      let stack =
        toml::from_str::<ResourceToml<PartialStackConfig>>(&current)
          .context("failed to parse pending Stack deletion")?;
      targets.insert(stack_target(&stack));
    }
  }
  Ok(targets)
}

fn maybe_extend(
  logs: &mut Vec<komodo_client::entities::update::Log>,
  log: Option<komodo_client::entities::update::Log>,
) {
  if let Some(log) = log {
    logs.push(log);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn gitops_execution_never_applies_delete_deltas() {
    assert!(!gitops_delete_allowed());
  }
}

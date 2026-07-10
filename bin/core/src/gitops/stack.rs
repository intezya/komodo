//! Stack reconciliation from prepared controller snapshots.

use database::mungos::{
  by_id::update_one_by_id,
  mongodb::bson::{doc, to_bson},
};
use komodo_client::entities::{
  stack::{Stack, StackState},
  update::Update,
  user::stack_user,
};

use crate::{
  api::execute::stack::{
    DeployIfChangedAction, deploy_services_with_options,
    resolve_deploy_if_changed_action, restart_services,
    update_deployed_contents_with_latest,
  },
  stack::remote::RemoteComposeContents,
  state::{db_client, stack_status_cache},
};

fn execution_error(error: mogh_error::Error) -> anyhow::Error {
  anyhow::Error::msg(error.error.to_string())
}

/// Reconciles only an existing running Stack and never refreshes Git itself.
pub(super) async fn reconcile_prepared_stack(
  stack: Stack,
  remote: RemoteComposeContents,
) -> anyhow::Result<Option<Update>> {
  let Some(status) = stack_status_cache().get(&stack.id).await else {
    return Ok(None);
  };
  if status.curr.state != StackState::Running
    || !remote.errored.is_empty()
  {
    return Ok(None);
  }
  let latest_contents = remote.successful;
  persist_latest_contents(
    &stack,
    &latest_contents,
    remote.hash,
    remote.message,
  )
  .await?;
  let action = match &stack.info.deployed_contents {
    Some(deployed_contents) => resolve_deploy_if_changed_action(
      deployed_contents,
      &latest_contents,
      &stack
        .info
        .latest_services
        .iter()
        .map(|service| service.service_name.clone())
        .collect::<Vec<_>>(),
    ),
    None => DeployIfChangedAction::FullDeploy,
  };
  let user = stack_user();
  let update = match action {
    DeployIfChangedAction::FullDeploy => {
      deploy_services_with_options(
        stack.name.clone(),
        Vec::new(),
        user,
        true,
        true,
      )
      .await
      .map_err(execution_error)?
    }
    DeployIfChangedAction::FullRestart => {
      let mut update =
        restart_services(stack.name.clone(), Vec::new(), user)
          .await
          .map_err(execution_error)?;
      if update.success {
        update_deployed_contents_with_latest(
          &stack.id,
          Some(latest_contents),
          &mut update,
        )
        .await;
      }
      update
    }
    DeployIfChangedAction::Services { deploy, restart } => {
      if deploy.is_empty() && restart.is_empty() {
        return Ok(None);
      }
      if !deploy.is_empty() {
        let update = deploy_services_with_options(
          stack.name.clone(),
          deploy,
          user,
          false,
          true,
        )
        .await
        .map_err(execution_error)?;
        if !update.success || restart.is_empty() {
          return Ok(Some(update));
        }
      }
      let mut update =
        restart_services(stack.name.clone(), restart, user)
          .await
          .map_err(execution_error)?;
      if update.success {
        update_deployed_contents_with_latest(
          &stack.id,
          Some(latest_contents),
          &mut update,
        )
        .await;
      }
      update
    }
  };
  Ok(Some(update))
}

async fn persist_latest_contents(
  stack: &Stack,
  contents: &[komodo_client::entities::stack::StackRemoteFileContents],
  hash: Option<String>,
  message: Option<String>,
) -> anyhow::Result<()> {
  update_one_by_id(
    &db_client().stacks,
    &stack.id,
    doc! {
      "$set": {
        "info.remote_contents": to_bson(contents)?,
        "info.remote_errors": to_bson(&Vec::<komodo_client::entities::FileContents>::new())?,
        "info.latest_hash": hash,
        "info.latest_message": message,
      }
    },
    None,
  )
  .await?;
  Ok(())
}

use std::time::Duration;

use anyhow::Context;
use database::mungos::{
  by_id::{find_one_by_id, update_one_by_id},
  mongodb::bson::to_document,
};
use komodo_client::entities::{
  Operation, ResourceTarget,
  action::Action,
  alerter::Alerter,
  build::Build,
  deployment::Deployment,
  komodo_timestamp,
  permission::PermissionLevel,
  procedure::Procedure,
  repo::Repo,
  server::Server,
  stack::Stack,
  swarm::Swarm,
  sync::ResourceSync,
  update::{Update, UpdateListItem, UpdateStatus},
  user::User,
};

use crate::{
  api::execute::{
    ExecuteRequest, alerter::get_authorized_send_alert_alerters,
    maintenance::ensure_admin_execution,
  },
  permission::get_check_permissions,
  resource,
  state::db_client,
};

use super::channel::update_channel;

pub fn make_update(
  target: impl Into<ResourceTarget>,
  operation: Operation,
  user: &User,
) -> Update {
  Update {
    start_ts: komodo_timestamp(),
    target: target.into(),
    operation,
    operator: user.id.clone(),
    success: true,
    ..Default::default()
  }
}

pub async fn add_update(
  mut update: Update,
) -> anyhow::Result<String> {
  update.id = db_client()
    .updates
    .insert_one(&update)
    .await
    .context("failed to insert update into db")?
    .inserted_id
    .as_object_id()
    .context("inserted_id is not object id")?
    .to_string();
  let id = update.id.clone();
  let update = update_list_item(update).await?;
  let _ = send_update(update).await;
  Ok(id)
}

pub async fn add_update_without_send(
  update: &Update,
) -> anyhow::Result<String> {
  let id = db_client()
    .updates
    .insert_one(update)
    .await
    .context("failed to insert update into db")?
    .inserted_id
    .as_object_id()
    .context("inserted_id is not object id")?
    .to_string();
  Ok(id)
}

pub async fn update_update(update: Update) -> anyhow::Result<()> {
  update_one_by_id(&db_client().updates, &update.id, database::mungos::update::Update::Set(to_document(&update)?), None)
    .await
    .context("failed to update the update on db. the update build process was deleted")?;
  let update = update_list_item(update).await?;
  let _ = send_update(update).await;
  Ok(())
}

async fn update_list_item(
  update: Update,
) -> anyhow::Result<UpdateListItem> {
  let username = if User::is_service_user(&update.operator) {
    update.operator.clone()
  } else {
    find_one_by_id(&db_client().users, &update.operator)
      .await
      .context("failed to query mongo for user")?
      .with_context(|| {
        format!("no user found with id {}", update.operator)
      })?
      .username
  };
  let update = UpdateListItem {
    id: update.id,
    operation: update.operation,
    start_ts: update.start_ts,
    success: update.success,
    operator: update.operator,
    target: update.target,
    status: update.status,
    version: update.version,
    other_data: update.other_data,
    username,
  };
  Ok(update)
}

async fn send_update(update: UpdateListItem) -> anyhow::Result<()> {
  update_channel().sender.lock().await.send(update)?;
  Ok(())
}

pub async fn init_execution_update(
  request: &ExecuteRequest,
  user: &User,
) -> anyhow::Result<Update> {
  macro_rules! init_execution_match {
    (
      resource: [$(($Variant:ident, $ResType:ident, $field:ident)),* $(,)?],
      batch: [$($BatchVariant:ident),* $(,)?],
      stack_service: [$(($StackVariant:ident, $ServiceOp:ident)),* $(,)?],
      system: [$($SysVariant:ident),* $(,)?],
    ) => {
      match &request {
        $(
          ExecuteRequest::$Variant(data) => (
            Operation::$Variant,
            ResourceTarget::$ResType(
              resource::get::<$ResType>(&data.$field).await?.id,
            ),
          ),
        )*
        $(
          ExecuteRequest::$BatchVariant(_data) => {
            return Ok(Default::default());
          }
        )*
        $(
          ExecuteRequest::$StackVariant(data) => (
            if !data.services.is_empty() {
              Operation::$ServiceOp
            } else {
              Operation::$StackVariant
            },
            ResourceTarget::Stack(
              resource::get::<Stack>(&data.stack).await?.id,
            ),
          ),
        )*
        // DeployStackIfChanged doesn't have a service variant
        ExecuteRequest::DeployStackIfChanged(data) => (
          Operation::DeployStack,
          ResourceTarget::Stack(
            resource::get::<Stack>(&data.stack).await?.id,
          ),
        ),
        $(
          ExecuteRequest::$SysVariant(_data) => {
            (Operation::$SysVariant, ResourceTarget::system())
          }
        )*
      }
    };
  }

  let (operation, target) = init_execution_match!(
    resource: [
      // Swarm
      (RemoveSwarmNodes, Swarm, swarm),
      (UpdateSwarmNode, Swarm, swarm),
      (RemoveSwarmStacks, Swarm, swarm),
      (RemoveSwarmServices, Swarm, swarm),
      (CreateSwarmConfig, Swarm, swarm),
      (RotateSwarmConfig, Swarm, swarm),
      (RemoveSwarmConfigs, Swarm, swarm),
      (CreateSwarmSecret, Swarm, swarm),
      (RotateSwarmSecret, Swarm, swarm),
      (RemoveSwarmSecrets, Swarm, swarm),
      // Server
      (StartContainer, Server, server),
      (RestartContainer, Server, server),
      (PauseContainer, Server, server),
      (UnpauseContainer, Server, server),
      (StopContainer, Server, server),
      (DestroyContainer, Server, server),
      (StartAllContainers, Server, server),
      (RestartAllContainers, Server, server),
      (PauseAllContainers, Server, server),
      (UnpauseAllContainers, Server, server),
      (StopAllContainers, Server, server),
      (PruneContainers, Server, server),
      (DeleteNetwork, Server, server),
      (PruneNetworks, Server, server),
      (DeleteImage, Server, server),
      (PruneImages, Server, server),
      (DeleteVolume, Server, server),
      (PruneVolumes, Server, server),
      (PruneDockerBuilders, Server, server),
      (PruneBuildx, Server, server),
      (PruneSystem, Server, server),
      // Deployment
      (Deploy, Deployment, deployment),
      (PullDeployment, Deployment, deployment),
      (StartDeployment, Deployment, deployment),
      (RestartDeployment, Deployment, deployment),
      (PauseDeployment, Deployment, deployment),
      (UnpauseDeployment, Deployment, deployment),
      (StopDeployment, Deployment, deployment),
      (DestroyDeployment, Deployment, deployment),
      // Build
      (RunBuild, Build, build),
      (CancelBuild, Build, build),
      // Repo
      (CloneRepo, Repo, repo),
      (PullRepo, Repo, repo),
      (BuildRepo, Repo, repo),
      (CancelRepoBuild, Repo, repo),
      // Procedure
      (RunProcedure, Procedure, procedure),
      // Action
      (RunAction, Action, action),
      // Resource Sync
      (RunSync, ResourceSync, sync),
      // Stack (simple)
      (RunStackService, Stack, stack),
      // Alerter
      (TestAlerter, Alerter, alerter),
    ],
    batch: [
      BatchDeploy,
      BatchDestroyDeployment,
      BatchRunBuild,
      BatchCloneRepo,
      BatchPullRepo,
      BatchBuildRepo,
      BatchRunProcedure,
      BatchRunAction,
      BatchDeployStack,
      BatchDeployStackIfChanged,
      BatchPullStack,
      BatchDestroyStack,
    ],
    stack_service: [
      (DeployStack, DeployStackService),
      (PullStack, PullStackService),
      (StartStack, StartStackService),
      (RestartStack, RestartStackService),
      (PauseStack, PauseStackService),
      (UnpauseStack, UnpauseStackService),
      (StopStack, StopStackService),
      (DestroyStack, DestroyStackService),
    ],
    system: [
      SendAlert,
      ClearRepoCache,
      BackupCoreDatabase,
      GlobalAutoUpdate,
      RotateAllServerKeys,
      RotateCoreKeys,
    ],
  );

  let mut update = make_update(target, operation, user);
  update.in_progress();

  // Hold off on even adding update for DeployStackIfChanged
  if !matches!(&request, ExecuteRequest::DeployStackIfChanged(_)) {
    // Don't actually send it here, let the handlers send it after they can set action state.
    update.id = add_update_without_send(&update).await?;
  }

  Ok(update)
}

pub async fn check_execute_permission_before_update(
  request: &ExecuteRequest,
  user: &User,
) -> mogh_error::Result<()> {
  macro_rules! check_permissions_match {
    (
      resource: [$(($Variant:ident, $ResType:ident, $field:ident)),* $(,)?],
      batch: [$($BatchVariant:ident),* $(,)?],
      stack_service: [$($StackVariant:ident),* $(,)?],
      system: [$($SysVariant:ident),* $(,)?],
    ) => {
      match request {
        $(
          ExecuteRequest::$Variant(data) => {
            get_check_permissions::<$ResType>(
              &data.$field,
              user,
              PermissionLevel::Execute.into(),
            )
            .await?;
          }
        )*
        $(
          ExecuteRequest::$BatchVariant(_) => {}
        )*
        $(
          ExecuteRequest::$StackVariant(data) => {
            get_check_permissions::<Stack>(
              &data.stack,
              user,
              PermissionLevel::Execute.into(),
            )
            .await?;
          }
        )*
        ExecuteRequest::DeployStackIfChanged(data) => {
          get_check_permissions::<Stack>(
            &data.stack,
            user,
            PermissionLevel::Execute.into(),
          )
          .await?;
        }
        $(
          ExecuteRequest::$SysVariant(_) => {
            check_system_execute_permission_before_update(
              request.clone(),
              user.clone(),
            )
            .await?;
          }
        )*
      }
    };
  }

  check_permissions_match!(
    resource: [
      (RemoveSwarmNodes, Swarm, swarm),
      (UpdateSwarmNode, Swarm, swarm),
      (RemoveSwarmStacks, Swarm, swarm),
      (RemoveSwarmServices, Swarm, swarm),
      (CreateSwarmConfig, Swarm, swarm),
      (RotateSwarmConfig, Swarm, swarm),
      (RemoveSwarmConfigs, Swarm, swarm),
      (CreateSwarmSecret, Swarm, swarm),
      (RotateSwarmSecret, Swarm, swarm),
      (RemoveSwarmSecrets, Swarm, swarm),
      (StartContainer, Server, server),
      (RestartContainer, Server, server),
      (PauseContainer, Server, server),
      (UnpauseContainer, Server, server),
      (StopContainer, Server, server),
      (DestroyContainer, Server, server),
      (StartAllContainers, Server, server),
      (RestartAllContainers, Server, server),
      (PauseAllContainers, Server, server),
      (UnpauseAllContainers, Server, server),
      (StopAllContainers, Server, server),
      (PruneContainers, Server, server),
      (DeleteNetwork, Server, server),
      (PruneNetworks, Server, server),
      (DeleteImage, Server, server),
      (PruneImages, Server, server),
      (DeleteVolume, Server, server),
      (PruneVolumes, Server, server),
      (PruneDockerBuilders, Server, server),
      (PruneBuildx, Server, server),
      (PruneSystem, Server, server),
      (Deploy, Deployment, deployment),
      (PullDeployment, Deployment, deployment),
      (StartDeployment, Deployment, deployment),
      (RestartDeployment, Deployment, deployment),
      (PauseDeployment, Deployment, deployment),
      (UnpauseDeployment, Deployment, deployment),
      (StopDeployment, Deployment, deployment),
      (DestroyDeployment, Deployment, deployment),
      (RunBuild, Build, build),
      (CancelBuild, Build, build),
      (CloneRepo, Repo, repo),
      (PullRepo, Repo, repo),
      (BuildRepo, Repo, repo),
      (CancelRepoBuild, Repo, repo),
      (RunProcedure, Procedure, procedure),
      (RunAction, Action, action),
      (RunSync, ResourceSync, sync),
      (RunStackService, Stack, stack),
      (TestAlerter, Alerter, alerter),
    ],
    batch: [
      BatchDeploy,
      BatchDestroyDeployment,
      BatchRunBuild,
      BatchCloneRepo,
      BatchPullRepo,
      BatchBuildRepo,
      BatchRunProcedure,
      BatchRunAction,
      BatchDeployStack,
      BatchDeployStackIfChanged,
      BatchPullStack,
      BatchDestroyStack,
    ],
    stack_service: [
      DeployStack,
      PullStack,
      StartStack,
      RestartStack,
      PauseStack,
      UnpauseStack,
      StopStack,
      DestroyStack,
    ],
    system: [
      SendAlert,
      ClearRepoCache,
      BackupCoreDatabase,
      GlobalAutoUpdate,
      RotateAllServerKeys,
      RotateCoreKeys,
    ],
  );

  Ok(())
}

fn check_system_execute_permission_before_update_with<
  AuthorizeSendAlert,
  AuthorizeSendAlertFuture,
>(
  request: ExecuteRequest,
  user: User,
  authorize_send_alert: AuthorizeSendAlert,
) -> impl std::future::Future<Output = mogh_error::Result<()>>
where
  AuthorizeSendAlert: FnOnce(
    komodo_client::api::execute::SendAlert,
    User,
  ) -> AuthorizeSendAlertFuture,
  AuthorizeSendAlertFuture:
    std::future::Future<Output = mogh_error::Result<Vec<Alerter>>>,
{
  async move {
    match request {
      ExecuteRequest::SendAlert(data) => {
        authorize_send_alert(data, user).await?;
      }
      ExecuteRequest::ClearRepoCache(_)
      | ExecuteRequest::BackupCoreDatabase(_)
      | ExecuteRequest::GlobalAutoUpdate(_)
      | ExecuteRequest::RotateAllServerKeys(_)
      | ExecuteRequest::RotateCoreKeys(_) => {
        ensure_admin_execution(&user)?;
      }
      _ => {}
    }

    Ok(())
  }
}

async fn check_system_execute_permission_before_update(
  request: ExecuteRequest,
  user: User,
) -> mogh_error::Result<()> {
  check_system_execute_permission_before_update_with(
    request,
    user,
    |send_alert, user| async move {
      get_authorized_send_alert_alerters(&send_alert, &user).await
    },
  )
  .await
}

#[cfg(test)]
async fn init_execution_update_after_permission_check_with<
  CheckPermissions,
  CheckPermissionsFuture,
  InitUpdate,
  InitUpdateFuture,
>(
  request: &ExecuteRequest,
  user: &User,
  check_permissions: CheckPermissions,
  init_update: InitUpdate,
) -> anyhow::Result<Update>
where
  CheckPermissions:
    FnOnce(&ExecuteRequest, &User) -> CheckPermissionsFuture,
  CheckPermissionsFuture:
    std::future::Future<Output = anyhow::Result<()>>,
  InitUpdate: FnOnce(&ExecuteRequest, &User) -> InitUpdateFuture,
  InitUpdateFuture:
    std::future::Future<Output = anyhow::Result<Update>>,
{
  check_permissions(request, user).await?;
  init_update(request, user).await
}

pub async fn init_execution_update_after_permission_check(
  request: &ExecuteRequest,
  user: &User,
) -> anyhow::Result<Update> {
  check_execute_permission_before_update(request, user)
    .await
    .map_err(|e| e.error)?;
  init_execution_update(request, user).await
}

pub async fn poll_update_until_complete(
  update_id: &str,
) -> anyhow::Result<Update> {
  loop {
    tokio::time::sleep(Duration::from_secs(1)).await;
    let update = find_one_by_id(&db_client().updates, update_id)
      .await?
      .context("No update found at given ID")?;
    if matches!(update.status, UpdateStatus::Complete) {
      return Ok(update);
    }
  }
}

#[cfg(test)]
mod tests {
  use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
  };

  use anyhow::anyhow;
  use komodo_client::api::execute::{
    BackupCoreDatabase, BatchDeploy, ClearRepoCache,
    GlobalAutoUpdate, RotateAllServerKeys, RotateCoreKeys, SendAlert,
    StartContainer,
  };
  use mogh_error::AddStatusCodeError;
  use reqwest::StatusCode;

  use super::*;

  #[tokio::test]
  async fn failed_preflight_does_not_initialize_update() {
    let init_calls = Arc::new(AtomicUsize::new(0));

    let err = init_execution_update_after_permission_check_with(
      &ExecuteRequest::StartContainer(StartContainer {
        server: "server-a".to_string(),
        container: "container-a".to_string(),
      }),
      &User::default(),
      |_, _| async { Err(anyhow!("permission denied")) },
      {
        let init_calls = init_calls.clone();
        |_, _| async move {
          init_calls.fetch_add(1, Ordering::Relaxed);
          Ok(Update::default())
        }
      },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("permission denied"));
    assert_eq!(init_calls.load(Ordering::Relaxed), 0);
  }

  #[tokio::test]
  async fn successful_preflight_initializes_update_once() {
    let init_calls = Arc::new(AtomicUsize::new(0));

    let _ = init_execution_update_after_permission_check_with(
      &ExecuteRequest::StartContainer(StartContainer {
        server: "server-a".to_string(),
        container: "container-a".to_string(),
      }),
      &User::default(),
      |_, _| async { Ok(()) },
      {
        let init_calls = init_calls.clone();
        |_, _| async move {
          init_calls.fetch_add(1, Ordering::Relaxed);
          Ok(Update::default())
        }
      },
    )
    .await
    .unwrap();

    assert_eq!(init_calls.load(Ordering::Relaxed), 1);
  }

  #[tokio::test]
  async fn batch_execute_requests_skip_permission_preflight() {
    check_execute_permission_before_update(
      &ExecuteRequest::BatchDeploy(BatchDeploy {
        pattern: "*".to_string(),
      }),
      &User::default(),
    )
    .await
    .unwrap();
  }

  #[tokio::test]
  async fn admin_only_system_execute_requests_require_preflight() {
    for request in [
      ExecuteRequest::ClearRepoCache(ClearRepoCache {}),
      ExecuteRequest::BackupCoreDatabase(BackupCoreDatabase {}),
      ExecuteRequest::GlobalAutoUpdate(GlobalAutoUpdate {
        skip_auto_update: false,
      }),
      ExecuteRequest::RotateAllServerKeys(RotateAllServerKeys {}),
      ExecuteRequest::RotateCoreKeys(RotateCoreKeys { force: false }),
    ] {
      let err = check_execute_permission_before_update(
        &request,
        &User::default(),
      )
      .await
      .unwrap_err();

      assert_eq!(err.status, StatusCode::FORBIDDEN);
      assert!(err.error.to_string().contains("admin only"));
    }
  }

  #[tokio::test]
  async fn admin_only_system_execute_preflight_returns_forbidden() {
    let err = check_execute_permission_before_update(
      &ExecuteRequest::BackupCoreDatabase(BackupCoreDatabase {}),
      &User::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(err.status, StatusCode::FORBIDDEN);
    assert!(err.error.to_string().contains("admin only"));
  }

  #[tokio::test]
  async fn admin_only_system_execute_requests_allow_admin() {
    let admin = User {
      admin: true,
      ..Default::default()
    };

    check_execute_permission_before_update(
      &ExecuteRequest::BackupCoreDatabase(BackupCoreDatabase {}),
      &admin,
    )
    .await
    .unwrap();
  }

  #[tokio::test]
  async fn failed_admin_preflight_does_not_initialize_update() {
    let init_calls = Arc::new(AtomicUsize::new(0));

    let err = init_execution_update_after_permission_check_with(
      &ExecuteRequest::BackupCoreDatabase(BackupCoreDatabase {}),
      &User::default(),
      |request, user| {
        let request = request.clone();
        let user = user.clone();
        async move {
          check_execute_permission_before_update(&request, &user)
            .await
            .map_err(|e| e.error)
        }
      },
      {
        let init_calls = init_calls.clone();
        |_, _| async move {
          init_calls.fetch_add(1, Ordering::Relaxed);
          Ok(Update::default())
        }
      },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("admin only"));
    assert_eq!(init_calls.load(Ordering::Relaxed), 0);
  }

  #[tokio::test]
  async fn send_alert_preflight_without_authorized_alerter_returns_bad_request()
   {
    let err = check_system_execute_permission_before_update_with(
      ExecuteRequest::SendAlert(SendAlert {
        level: Default::default(),
        message: "test".to_string(),
        details: String::new(),
        alerters: vec![String::from("alerter-a")],
      }),
      User::default(),
      |_, _| async {
        Err(
          anyhow!("no authorized alerters")
            .status_code(StatusCode::BAD_REQUEST),
        )
      },
    )
    .await
    .unwrap_err();

    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(err.error.to_string().contains("no authorized alerters"));
  }

  #[tokio::test]
  async fn failed_send_alert_preflight_does_not_initialize_update() {
    let init_calls = Arc::new(AtomicUsize::new(0));

    let err = init_execution_update_after_permission_check_with(
      &ExecuteRequest::SendAlert(SendAlert {
        level: Default::default(),
        message: "test".to_string(),
        details: String::new(),
        alerters: vec![String::from("alerter-a")],
      }),
      &User::default(),
      |request, user| {
        let request = request.clone();
        let user = user.clone();
        async move {
          check_system_execute_permission_before_update_with(
            request,
            user,
            |_, _| async {
              Err(
                anyhow!("no authorized alerters")
                  .status_code(StatusCode::BAD_REQUEST),
              )
            },
          )
          .await
          .map_err(|e| e.error)
        }
      },
      {
        let init_calls = init_calls.clone();
        |_, _| async move {
          init_calls.fetch_add(1, Ordering::Relaxed);
          Ok(Update::default())
        }
      },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("no authorized alerters"));
    assert_eq!(init_calls.load(Ordering::Relaxed), 0);
  }

  #[tokio::test]
  async fn authorized_send_alert_preflight_initializes_update_once() {
    let init_calls = Arc::new(AtomicUsize::new(0));

    let _ = init_execution_update_after_permission_check_with(
      &ExecuteRequest::SendAlert(SendAlert {
        level: Default::default(),
        message: "test".to_string(),
        details: String::new(),
        alerters: vec![String::from("alerter-a")],
      }),
      &User::default(),
      |request, user| {
        let request = request.clone();
        let user = user.clone();
        async move {
          check_system_execute_permission_before_update_with(
            request,
            user,
            |_, _| async { Ok(vec![Alerter::default()]) },
          )
          .await
          .map_err(|e| e.error)
        }
      },
      {
        let init_calls = init_calls.clone();
        |_, _| async move {
          init_calls.fetch_add(1, Ordering::Relaxed);
          Ok(Update::default())
        }
      },
    )
    .await
    .unwrap();

    assert_eq!(init_calls.load(Ordering::Relaxed), 1);
  }
}

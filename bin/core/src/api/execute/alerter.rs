use anyhow::{Context, anyhow};
use formatting::format_serror;
use futures_util::{
  StreamExt, TryStreamExt, stream::FuturesUnordered,
};
use komodo_client::{
  api::execute::{SendAlert, TestAlerter},
  entities::{
    alert::{Alert, AlertData, AlertDataVariant, SeverityLevel},
    alerter::Alerter,
    komodo_timestamp,
    permission::PermissionLevel,
    user::User,
  },
};
use mogh_error::AddStatusCodeError;
use mogh_resolver::Resolve;
use reqwest::StatusCode;

use crate::{
  alert::send_alert_to_alerter, helpers::update::update_update,
  permission::get_check_permissions, resource::list_full_for_user,
};

use super::ExecuteArgs;

async fn get_authorized_send_alert_alerters_with<
  CheckExecute,
  CheckExecuteFuture,
>(
  send_alert: &SendAlert,
  user: &User,
  alerters: Vec<Alerter>,
  check_execute: CheckExecute,
) -> mogh_error::Result<Vec<Alerter>>
where
  CheckExecute: Fn(Alerter) -> CheckExecuteFuture,
  CheckExecuteFuture:
    std::future::Future<Output = anyhow::Result<Alerter>>,
{
  let alerters = alerters
    .into_iter()
    .filter(|alerter| {
      alerter.config.enabled
        && (send_alert.alerters.is_empty()
          || send_alert.alerters.contains(&alerter.name)
          || send_alert.alerters.contains(&alerter.id))
        && (alerter.config.alert_types.is_empty()
          || alerter
            .config
            .alert_types
            .contains(&AlertDataVariant::Custom))
    })
    .collect::<Vec<_>>();

  let alerters = if user.admin {
    alerters
  } else {
    alerters
      .into_iter()
      .map(check_execute)
      .collect::<FuturesUnordered<_>>()
      .collect::<Vec<_>>()
      .await
      .into_iter()
      .flatten()
      .collect()
  };

  if alerters.is_empty() {
    return Err(anyhow!(
      "Could not find any valid alerters to send to, this required Execute permissions on the Alerter"
    )
    .status_code(StatusCode::BAD_REQUEST));
  }

  Ok(alerters)
}

pub(crate) async fn get_authorized_send_alert_alerters(
  send_alert: &SendAlert,
  user: &User,
) -> mogh_error::Result<Vec<Alerter>> {
  let alerters = list_full_for_user::<Alerter>(
    Default::default(),
    user,
    PermissionLevel::Read.into(),
    &[],
  )
  .await?;

  get_authorized_send_alert_alerters_with(
    send_alert,
    user,
    alerters,
    |alerter| async move {
      get_check_permissions::<Alerter>(
        &alerter.id,
        user,
        PermissionLevel::Execute.into(),
      )
      .await
    },
  )
  .await
}

impl Resolve<ExecuteArgs> for TestAlerter {
  #[instrument(
    "TestAlerter",
    skip_all,
    fields(
      task_id = task_id.to_string(),
      operator = user.id,
      update_id = update.id,
      alerter = self.alerter,
    )
  )]
  async fn resolve(
    self,
    ExecuteArgs {
      user,
      update,
      task_id,
    }: &ExecuteArgs,
  ) -> Result<Self::Response, Self::Error> {
    let alerter = get_check_permissions::<Alerter>(
      &self.alerter,
      user,
      PermissionLevel::Execute.into(),
    )
    .await?;

    let mut update = update.clone();

    if !alerter.config.enabled {
      update.push_error_log(
        "Test Alerter",
        String::from(
          "Alerter is disabled. Enable the Alerter to send alerts.",
        ),
      );
      update.finalize();
      update_update(update.clone()).await?;
      return Ok(update);
    }

    let ts = komodo_timestamp();

    let alert = Alert {
      id: Default::default(),
      ts,
      resolved: true,
      level: SeverityLevel::Ok,
      target: update.target.clone(),
      data: AlertData::Test {
        id: alerter.id.clone(),
        name: alerter.name.clone(),
      },
      resolved_ts: Some(ts),
    };

    if let Err(e) = send_alert_to_alerter(&alerter, &alert).await {
      update.push_error_log("Test Alerter", format_serror(&e.into()));
    } else {
      update.push_simple_log("Test Alerter", String::from("Alert sent successfully. It should be visible at your alerting destination."));
    };

    update.finalize();
    update_update(update.clone()).await?;

    Ok(update)
  }
}

//

impl Resolve<ExecuteArgs> for SendAlert {
  #[instrument(
    "SendAlert",
    skip_all,
    fields(
      task_id = task_id.to_string(),
      operator = user.id,
      update_id = update.id,
      request = format!("{self:?}"),
    )
  )]
  async fn resolve(
    self,
    ExecuteArgs {
      user,
      update,
      task_id,
    }: &ExecuteArgs,
  ) -> Result<Self::Response, Self::Error> {
    let alerters =
      get_authorized_send_alert_alerters(&self, user).await?;

    let mut update = update.clone();

    let ts = komodo_timestamp();

    let alert = Alert {
      id: Default::default(),
      ts,
      resolved: true,
      level: self.level,
      target: update.target.clone(),
      data: AlertData::Custom {
        message: self.message,
        details: self.details,
      },
      resolved_ts: Some(ts),
    };

    update.push_simple_log(
      "Send alert",
      serde_json::to_string_pretty(&alert)
        .context("Failed to serialize alert to JSON")?,
    );

    if let Err(e) = alerters
      .iter()
      .map(|alerter| send_alert_to_alerter(alerter, &alert))
      .collect::<FuturesUnordered<_>>()
      .try_collect::<Vec<_>>()
      .await
    {
      update.push_error_log("Send Error", format_serror(&e.into()));
    };

    update.finalize();
    update_update(update.clone()).await?;

    Ok(update)
  }
}

#[cfg(test)]
mod tests {
  use anyhow::anyhow;
  use komodo_client::api::execute::SendAlert;

  use super::*;

  #[tokio::test]
  async fn send_alert_authorization_requires_executable_alerter() {
    let mut alerter = Alerter::default();
    alerter.id = String::from("alerter-1");
    alerter.name = String::from("alerter-1");
    alerter.config.enabled = true;

    let err = get_authorized_send_alert_alerters_with(
      &SendAlert {
        level: Default::default(),
        message: String::from("test"),
        details: String::new(),
        alerters: vec![String::from("alerter-1")],
      },
      &User::default(),
      vec![alerter],
      |_alerter| async { Err(anyhow!("permission denied")) },
    )
    .await
    .unwrap_err();

    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(
      err
        .error
        .to_string()
        .contains("Could not find any valid alerters")
    );
  }

  #[tokio::test]
  async fn send_alert_authorization_keeps_authorized_alerter() {
    let mut alerter = Alerter::default();
    alerter.id = String::from("alerter-1");
    alerter.name = String::from("alerter-1");
    alerter.config.enabled = true;

    let alerters = get_authorized_send_alert_alerters_with(
      &SendAlert {
        level: Default::default(),
        message: String::from("test"),
        details: String::new(),
        alerters: vec![String::from("alerter-1")],
      },
      &User::default(),
      vec![alerter.clone()],
      |_alerter| {
        let alerter = alerter.clone();
        async move { Ok(alerter) }
      },
    )
    .await
    .unwrap();

    assert_eq!(alerters.len(), 1);
    assert_eq!(alerters[0].id, "alerter-1");
  }
}

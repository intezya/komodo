use futures_util::{StreamExt, stream::FuturesUnordered};
use komodo_client::entities::config::periphery::Command;
use tracing::Instrument;

use crate::{
  config::periphery_args,
  state::{core_public_keys, periphery_keys},
};

#[macro_use]
extern crate tracing;

mod api;
mod config;
mod connection;
mod docker;
mod helpers;
mod stack;
mod state;
mod stats;
mod terminal;

async fn app() -> anyhow::Result<()> {
  dotenvy::dotenv().ok();
  let config = config::periphery_config();
  mogh_logger::init(&config.logging)?;

  let startup_span = info_span!("PeripheryStartup");

  let mut handles = async {
    info!("Komodo Periphery version: v{}", env!("CARGO_PKG_VERSION"));

    if config.pretty_startup_config {
      info!("{:#?}", config.sanitized());
    } else {
      info!("{:?}", config.sanitized());
    }

    // Init + log public key. Will crash if invalid private key here.
    info!("Public Key: {}", periphery_keys().load().public);

    // Init core public keys. Will crash if invalid core public keys here.
    core_public_keys();

    rustls::crypto::aws_lc_rs::default_provider()
      .install_default()
      .expect("Failed to install default crypto provider");

    stats::spawn_polling_thread();
    docker::stats::spawn_polling_thread();

    let handles = FuturesUnordered::new();

    // Spawn client side connections
    if !config.core_addresses.is_empty() && config.connect_as.is_empty()
    {
      warn!(
        "'core_addresses' are defined for outbound connection, but missing 'connect_as' (PERIPHERY_CONNECT_AS)."
      );
    } else {
      for address in &config.core_addresses {
        match connection::client::handler(address).await {
          Ok(handle) => handles.push(handle),
          Err(e) => {
            error!("Failed to start outbound connection to {address} | {e:#}");
          }
        }
      }
    }

    // Spawn server connection handler.
    if config.server_enabled() {
      match connection::server::run().await {
        Ok(handle) => handles.push(handle),
        Err(e) => {
          error!("Failed to run inbound connection server | {e:#}");
        }
      }
    }

    handles
  }.instrument(startup_span).await;

  // Watch the threads
  while let Some(res) = handles.next().await {
    match res {
      Ok(Err(e)) => {
        error!("CONNECTION ERROR: {e:#}");
      }
      Err(e) => {
        error!("SPAWN ERROR: {e:#}");
      }
      Ok(Ok(())) => {}
    }
  }

  Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  // Handle `periphery key gen` and `periphery key compute <private-key>`
  if let Some(Command::Key { command }) = &periphery_args().command {
    return mogh_pki::cli::handle(command, mogh_pki::PkiKind::Mutual)
      .await;
  }

  let mut term_signal = tokio::signal::unix::signal(
    tokio::signal::unix::SignalKind::terminate(),
  )?;
  tokio::select! {
    res = tokio::spawn(app()) => return res?,
    _ = term_signal.recv() => {
      info!("Exiting all active Terminals for shutdown");
      terminal::delete_all_terminals().await;
      Ok(())
    },
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
  fn compose_deploy_keeps_failed_pull_log_when_up_succeeds() {
    let res = crate::api::compose_deploy_response_from_logs(vec![
      log("Compose Pull", false),
      log("Compose Up", true),
    ]);

    assert!(res.deployed);
    assert_eq!(res.logs.len(), 2);
    assert_eq!(res.logs[0].stage, "Compose Pull");
    assert!(!res.logs[0].success);
  }

  #[test]
  fn compose_deploy_requires_compose_up() {
    let res =
      crate::api::compose_deploy_response_from_logs(vec![log(
        "Compose Pull",
        false,
      )]);

    assert!(!res.deployed);
  }

  #[test]
  fn compose_deploy_is_false_when_up_fails() {
    let res = crate::api::compose_deploy_response_from_logs(vec![
      log("Compose Pull", false),
      log("Compose Up", false),
    ]);

    assert!(!res.deployed);
  }
}

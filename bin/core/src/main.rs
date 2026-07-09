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

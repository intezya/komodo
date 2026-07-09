use std::{
  sync::{
    Arc, Mutex,
    atomic::{self, AtomicBool},
  },
  time::{Duration, Instant},
};

use anyhow::{Context as _, anyhow};
use database::mungos::{by_id::update_one_by_id, mongodb::bson::doc};
use encoding::{
  CastBytes as _, Decode as _, EncodedJsonMessage, EncodedResponse,
  WithChannel,
};
use komodo_client::entities::{
  builder::{AwsBuilderConfig, UrlBuilderConfig},
  optional_str,
  server::Server,
};
use mogh_cache::CloneCache;
use mogh_error::serror_into_anyhow_error;
use periphery_client::transport::{
  EncodedTransportMessage, ResponseMessage, TransportMessage,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use transport::{
  auth::{
    ConnectionIdentifiers, LoginFlow, LoginFlowArgs,
    PublicKeyValidator,
  },
  channel::{BufferedReceiver, Sender, buffered_channel},
  websocket::{
    Websocket, WebsocketMessage, WebsocketReceiver as _,
    WebsocketSender as _,
  },
};
use uuid::Uuid;

use crate::{
  config::{core_keys, periphery_public_keys},
  state::db_client,
};

pub mod client;
pub mod server;

pub(crate) const DUPLICATE_CONNECTION_STALE_AFTER: Duration =
  Duration::from_secs(12);

#[derive(Default)]
pub struct PeripheryConnections(
  CloneCache<String, Arc<PeripheryConnection>>,
);

impl PeripheryConnections {
  /// Create a replacement candidate without publishing it for routing.
  pub async fn prepare(
    &self,
    server_id: String,
    args: PeripheryConnectionArgs<'_>,
  ) -> (
    Arc<PeripheryConnection>,
    BufferedReceiver<EncodedTransportMessage>,
  ) {
    if let Some(existing_connection) = self.0.get(&server_id).await {
      existing_connection.with_new_args(args)
    } else {
      PeripheryConnection::new(args)
    }
  }

  /// Publish an authenticated replacement for command routing.
  pub async fn publish(
    &self,
    server_id: String,
    connection: Arc<PeripheryConnection>,
  ) {
    if let Some(existing_connection) = self.0.remove(&server_id).await
    {
      existing_connection.deactivate();
    }

    self.0.insert(server_id, connection).await;
  }

  /// Insert a recreated connection.
  /// Ensures the fields which must be persisted between
  /// connection recreation are carried over.
  pub async fn insert(
    &self,
    server_id: String,
    args: PeripheryConnectionArgs<'_>,
  ) -> (
    Arc<PeripheryConnection>,
    BufferedReceiver<EncodedTransportMessage>,
  ) {
    let (connection, receiver) = if let Some(existing_connection) =
      self.0.remove(&server_id).await
    {
      existing_connection.replace_with_new_args(args)
    } else {
      PeripheryConnection::new(args)
    };

    self.0.insert(server_id, connection.clone()).await;

    (connection, receiver)
  }

  pub async fn get(
    &self,
    server_id: &String,
  ) -> Option<Arc<PeripheryConnection>> {
    self.0.get(server_id).await
  }

  /// Remove and cancel connection
  pub async fn remove(
    &self,
    server_id: &String,
  ) -> Option<Arc<PeripheryConnection>> {
    self
      .0
      .remove(server_id)
      .await
      .inspect(|connection| connection.cancel())
  }
}

/// The configurable args of a connection
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeripheryConnectionArgs<'a> {
  /// Usually the server id
  pub id: &'a str,
  pub address: Option<&'a str>,
  periphery_public_key: Option<&'a str>,
  /// V1 legacy support.
  /// Only possible for Core -> Periphery.
  passkey: Option<&'a str>,
}

impl PublicKeyValidator for PeripheryConnectionArgs<'_> {
  type ValidationResult = String;
  #[instrument("ValidatePeripheryPublicKey", skip(self))]
  async fn validate(
    &self,
    public_key: String,
  ) -> anyhow::Result<Self::ValidationResult> {
    let invalid_error = || {
      spawn_update_attempted_public_key(
        self.id.to_string(),
        Some(public_key.clone()),
      );
      anyhow!("{public_key} is invalid")
        .context(
          "Ensure public key matches configured Periphery Public Key",
        )
        .context("Core failed to validate Periphery public key")
    };
    let core_to_periphery = self.address.is_some();
    match (self.periphery_public_key, core_to_periphery) {
      // The key matches expected.
      (Some(expected), _) if public_key == expected => Ok(public_key),
      // Explicit auth failed.
      (Some(_), _) => Err(invalid_error()),
      // Core -> Periphery connections with no explicit
      // Periphery public key are not validated.
      (None, true) => Ok(public_key),
      // Periphery -> Core connections with no explicit
      // Periphery public key can fall back to Core config `periphery_public_keys` if defined.
      (None, false) => {
        let expected =
          periphery_public_keys().ok_or_else(invalid_error)?;
        if expected
          .iter()
          .any(|expected| public_key == expected.as_str())
        {
          Ok(public_key)
        } else {
          Err(invalid_error())
        }
      }
    }
  }
}

impl<'a> PeripheryConnectionArgs<'a> {
  pub fn from_server(server: &'a Server) -> Self {
    Self {
      id: &server.id,
      address: optional_str(&server.config.address),
      periphery_public_key: optional_str(&server.info.public_key),
      passkey: optional_str(&server.config.passkey),
    }
  }

  pub fn from_url_builder(
    id: &'a str,
    config: &'a UrlBuilderConfig,
  ) -> Self {
    Self {
      id,
      address: optional_str(&config.address),
      periphery_public_key: optional_str(
        &config.periphery_public_key,
      ),
      passkey: optional_str(&config.passkey),
    }
  }

  pub fn from_aws_builder(
    id: &'a str,
    address: &'a str,
    config: &'a AwsBuilderConfig,
  ) -> Self {
    Self {
      id,
      address: Some(address),
      periphery_public_key: optional_str(
        &config.periphery_public_key,
      ),
      passkey: None,
    }
  }

  pub fn to_owned(self) -> OwnedPeripheryConnectionArgs {
    OwnedPeripheryConnectionArgs {
      id: self.id.to_string(),
      address: self.address.map(str::to_string),
      periphery_public_key: self
        .periphery_public_key
        .map(str::to_string),
      passkey: self.passkey.map(str::to_string),
    }
  }

  pub fn matches<'b>(
    self,
    args: impl Into<PeripheryConnectionArgs<'b>>,
  ) -> bool {
    self == args.into()
  }
}

#[derive(Debug, Clone)]
pub struct OwnedPeripheryConnectionArgs {
  /// Usually the Server id.
  pub id: String,
  /// Specify outbound connection address.
  /// Inbound connections have this as None
  pub address: Option<String>,
  /// The public key to expect Periphery to have.
  /// If None, must have 'periphery_public_keys' set
  /// in Core config, or will error
  pub periphery_public_key: Option<String>,
  /// V1 legacy support.
  /// Only possible for Core -> Periphery connection.
  pub passkey: Option<String>,
}

impl OwnedPeripheryConnectionArgs {
  pub fn borrow(&self) -> PeripheryConnectionArgs<'_> {
    PeripheryConnectionArgs {
      id: &self.id,
      address: self.address.as_deref(),
      periphery_public_key: self.periphery_public_key.as_deref(),
      passkey: self.passkey.as_deref(),
    }
  }
}

impl From<PeripheryConnectionArgs<'_>>
  for OwnedPeripheryConnectionArgs
{
  fn from(value: PeripheryConnectionArgs<'_>) -> Self {
    value.to_owned()
  }
}

impl<'a> From<&'a OwnedPeripheryConnectionArgs>
  for PeripheryConnectionArgs<'a>
{
  fn from(value: &'a OwnedPeripheryConnectionArgs) -> Self {
    value.borrow()
  }
}

/// Sends None as InProgress ping.
pub type ResponseChannels =
  CloneCache<Uuid, Sender<EncodedResponse<EncodedJsonMessage>>>;

pub type TerminalChannels =
  CloneCache<Uuid, Sender<anyhow::Result<Vec<u8>>>>;

#[derive(Debug)]
pub struct PeripheryConnection {
  /// The connection args
  pub args: OwnedPeripheryConnectionArgs,
  /// Send and receive bytes over the connection socket.
  pub sender: Sender<EncodedTransportMessage>,
  /// Cancel the connection
  pub cancel: CancellationToken,
  /// Whether Periphery is currently connected.
  pub connected: AtomicBool,
  /// Last observed read-side activity from Periphery.
  pub last_activity: Mutex<Instant>,
  // These fields must be maintained if new connection replaces old
  // at the same server id.
  /// Stores latest connection error
  pub error: Arc<RwLock<Option<mogh_error::Serror>>>,
  /// Forward bytes from Periphery to response channel handlers.
  pub responses: Arc<ResponseChannels>,
  /// Forward bytes from Periphery to terminal channel handlers.
  pub terminals: Arc<TerminalChannels>,
}

impl PeripheryConnection {
  pub fn new(
    args: impl Into<OwnedPeripheryConnectionArgs>,
  ) -> (
    Arc<PeripheryConnection>,
    BufferedReceiver<EncodedTransportMessage>,
  ) {
    let (sender, receiever) = buffered_channel();
    (
      PeripheryConnection {
        sender,
        args: args.into(),
        cancel: CancellationToken::new(),
        connected: AtomicBool::new(false),
        last_activity: Mutex::new(Instant::now()),
        error: Default::default(),
        responses: Default::default(),
        terminals: Default::default(),
      }
      .into(),
      receiever,
    )
  }

  pub fn with_new_args(
    &self,
    args: impl Into<OwnedPeripheryConnectionArgs>,
  ) -> (
    Arc<PeripheryConnection>,
    BufferedReceiver<EncodedTransportMessage>,
  ) {
    let (sender, receiever) = buffered_channel();
    (
      PeripheryConnection {
        sender,
        args: args.into(),
        cancel: CancellationToken::new(),
        connected: AtomicBool::new(false),
        last_activity: Mutex::new(Instant::now()),
        error: Default::default(),
        responses: self.responses.clone(),
        terminals: self.terminals.clone(),
      }
      .into(),
      receiever,
    )
  }

  pub fn replace_with_new_args(
    &self,
    args: impl Into<OwnedPeripheryConnectionArgs>,
  ) -> (
    Arc<PeripheryConnection>,
    BufferedReceiver<EncodedTransportMessage>,
  ) {
    self.deactivate();
    let (sender, receiever) = buffered_channel();
    (
      PeripheryConnection {
        sender,
        args: args.into(),
        cancel: CancellationToken::new(),
        connected: AtomicBool::new(false),
        last_activity: Mutex::new(Instant::now()),
        error: self.error.clone(),
        responses: self.responses.clone(),
        terminals: self.terminals.clone(),
      }
      .into(),
      receiever,
    )
  }

  #[instrument(
    "StandardPeripheryLoginFlow",
    skip(self, socket, identifiers),
    fields(expected_public_key = self.args.periphery_public_key)
  )]
  pub async fn handle_login<W: Websocket, L: LoginFlow>(
    &self,
    socket: &mut W,
    identifiers: ConnectionIdentifiers<'_>,
    should_close: bool,
  ) -> anyhow::Result<()> {
    L::login(LoginFlowArgs {
      socket,
      identifiers,
      private_key: core_keys().load().private.as_str(),
      public_key_validator: self.args.borrow(),
      should_close,
    })
    .await?;
    // Clear attempted public key after successful login
    spawn_update_attempted_public_key(self.args.id.clone(), None);
    Ok(())
  }

  pub async fn handle_socket<W: Websocket>(
    &self,
    socket: W,
    receiver: &mut BufferedReceiver<EncodedTransportMessage>,
  ) {
    let cancel = self.cancel.child_token();

    self.set_connected(true);
    self.clear_error().await;

    let (mut ws_write, mut ws_read) = socket.split();

    ws_read.set_cancel(cancel.clone());
    receiver.set_cancel(cancel.clone());

    let forward_writes = async {
      loop {
        let message = match tokio::time::timeout(
          Duration::from_secs(5),
          receiver.recv(),
        )
        .await
        {
          Ok(Ok(message)) => message,
          Ok(Err(_)) => break,
          // Handle sending Ping
          Err(_) => {
            if let Err(e) = ws_write.ping().await {
              self.set_error(e).await;
              break;
            }
            continue;
          }
        };
        match ws_write.send(message.into_bytes()).await {
          Ok(_) => receiver.clear_buffer(),
          Err(e) => {
            self.set_error(e).await;
            break;
          }
        }
      }
      // Cancel again if not already
      let _ = ws_write.close().await;
      cancel.cancel();
    };

    let handle_reads = async {
      loop {
        match tokio::time::timeout(
          Duration::from_secs(10),
          ws_read.recv(),
        )
        .await
        .context("Timed out waiting for Ping")
        {
          Ok(Ok(WebsocketMessage::Message(message))) => {
            self.mark_activity();
            match message.decode() {
              Ok(message) => {
                self.handle_incoming_message(message).await
              }
              Err(e) => {
                self.set_error(e).await;
                break;
              }
            }
          }
          Ok(Ok(WebsocketMessage::Ping)) => self.mark_activity(),
          Ok(Ok(WebsocketMessage::Close)) => {
            self.set_error(anyhow!("Connection closed")).await;
            break;
          }
          Ok(Ok(WebsocketMessage::Closed)) => {
            self
              .set_error(anyhow!("Connection already closed"))
              .await;
            break;
          }
          Err(e) => {
            self.set_error(e).await;
            break;
          }
          Ok(Err(e)) => {
            self.set_error(e).await;
            break;
          }
        }
      }
      // Cancel again if not already
      cancel.cancel();
    };

    tokio::join!(forward_writes, handle_reads);

    self.set_connected(false);
  }

  pub async fn handle_incoming_message(
    &self,
    message: TransportMessage,
  ) {
    match message {
      TransportMessage::Response(data) => {
        match data.decode().map(ResponseMessage::into_inner) {
          Ok(WithChannel { channel, data }) => {
            let Some(response_channel) =
              self.responses.get(&channel).await
            else {
              warn!(
                "Failed to forward Response message | No response channel found at {channel}"
              );
              return;
            };
            if let Err(e) = response_channel.send(data).await {
              warn!(
                "Failed to forward Response | Response channel failure at {channel} | {e:#}"
              );
            }
          }
          Err(e) => {
            warn!("Failed to read Response message | {e:#}");
          }
        }
      }
      TransportMessage::Terminal(data) => match data.decode() {
        Ok(WithChannel {
          channel: channel_id,
          data,
        }) => {
          let Some(channel) = self.terminals.get(&channel_id).await
          else {
            warn!(
              "Failed to forward Terminal message | No terminal channel found at {channel_id}"
            );
            return;
          };
          if let Err(e) = channel.send(data).await {
            warn!(
              "Failed to forward Terminal message | Channel failure at {channel_id} | {e:#}"
            );
          }
        }
        Err(e) => {
          warn!("Failed to read Terminal message | {e:#}");
        }
      },
      //
      other => {
        warn!("Received unexpected transport message | {other:?}");
      }
    }
  }

  pub fn set_connected(&self, connected: bool) {
    self.connected.store(connected, atomic::Ordering::Relaxed);
    if connected {
      self.mark_activity();
    }
  }

  pub fn connected(&self) -> bool {
    self.connected.load(atomic::Ordering::Relaxed)
  }

  pub fn mark_activity(&self) {
    *self.last_activity.lock().unwrap() = Instant::now();
  }

  pub fn should_reject_duplicate_connection(&self) -> bool {
    self.connected()
      && self.last_activity.lock().unwrap().elapsed()
        <= DUPLICATE_CONNECTION_STALE_AFTER
  }

  /// Polls connected 3 times (500ms in between) before bailing.
  pub async fn bail_if_not_connected(&self) -> anyhow::Result<()> {
    const POLL_TIMES: usize = 3;
    for i in 0..POLL_TIMES {
      if self.connected() {
        return Ok(());
      }
      if i < POLL_TIMES - 1 {
        tokio::time::sleep(Duration::from_millis(500)).await;
      }
    }
    if let Some(e) = self.error().await {
      Err(serror_into_anyhow_error(e))
    } else {
      Err(anyhow!("Server is not currently connected"))
    }
  }

  pub async fn error(&self) -> Option<mogh_error::Serror> {
    self.error.read().await.clone()
  }

  pub async fn set_error(&self, e: anyhow::Error) {
    let mut error = self.error.write().await;
    *error = Some(e.into());
  }

  pub async fn clear_error(&self) {
    let mut error = self.error.write().await;
    *error = None;
  }

  pub fn cancel(&self) {
    self.cancel.cancel();
  }

  pub fn deactivate(&self) {
    self.set_connected(false);
    self.cancel();
  }
}

/// Spawn task to set the 'attempted_public_key'
/// for easy manual connection acceptance later on.
fn spawn_update_attempted_public_key(
  id: String,
  public_key: impl Into<Option<String>>,
) {
  let public_key = public_key.into();
  tokio::spawn(async move {
    if let Err(e) = update_one_by_id(
      &db_client().servers,
      &id,
      doc! {
        "$set": {
          "info.attempted_public_key": &public_key.as_deref().unwrap_or_default(),
        }
      },
      None,
    )
    .await
    {
      warn!(
        "Failed to update attempted public_key for Server {id} | {e:?}"
      );
    };
  });
}

#[cfg(test)]
mod tests {
  use std::{
    sync::Arc,
    time::{Duration, Instant},
  };

  use anyhow::anyhow;
  use periphery_client::transport::LoginMessage;
  use tokio::time::timeout;

  use super::{
    DUPLICATE_CONNECTION_STALE_AFTER, PeripheryConnection,
    PeripheryConnectionArgs, PeripheryConnections,
  };

  fn test_args<'a>(id: &'a str) -> PeripheryConnectionArgs<'a> {
    PeripheryConnectionArgs {
      id,
      address: None,
      periphery_public_key: None,
      passkey: None,
    }
  }

  #[test]
  fn duplicate_reconnect_rejects_only_recently_active_connections() {
    let (connection, _receiver) =
      PeripheryConnection::new(test_args("server-1"));

    connection.set_connected(true);
    assert!(connection.should_reject_duplicate_connection());

    *connection.last_activity.lock().unwrap() = Instant::now()
      - DUPLICATE_CONNECTION_STALE_AFTER
      - Duration::from_secs(1);

    assert!(!connection.should_reject_duplicate_connection());
  }

  #[tokio::test]
  async fn prepare_does_not_replace_active_existing_connection() {
    let connections = PeripheryConnections::default();
    let server_id = String::from("server-1");

    let (old_connection, mut old_receiver) = connections
      .prepare(server_id.clone(), test_args("server-1"))
      .await;
    connections
      .publish(server_id.clone(), old_connection.clone())
      .await;
    old_connection.set_connected(true);

    let (candidate, mut candidate_receiver) = connections
      .prepare(server_id.clone(), test_args("server-1"))
      .await;

    let current = connections.get(&server_id).await.unwrap();
    assert!(Arc::ptr_eq(&current, &old_connection));
    assert!(!Arc::ptr_eq(&current, &candidate));
    assert!(
      !old_connection.cancel.is_cancelled(),
      "creating a replacement candidate must not evict the published connection before auth succeeds"
    );

    current
      .sender
      .send_message(LoginMessage::Success)
      .await
      .unwrap();

    timeout(Duration::from_millis(100), old_receiver.recv())
      .await
      .expect("old connection should remain routed after prepare")
      .unwrap();

    assert!(
      timeout(Duration::from_millis(100), candidate_receiver.recv())
        .await
        .is_err(),
      "unpublished replacement candidate should not receive routed messages"
    );
  }

  #[tokio::test]
  async fn failed_auth_without_publish_leaves_old_connection_routed()
  {
    let connections = PeripheryConnections::default();
    let server_id = String::from("server-1");

    let (old_connection, mut old_receiver) = connections
      .prepare(server_id.clone(), test_args("server-1"))
      .await;
    connections
      .publish(server_id.clone(), old_connection.clone())
      .await;
    old_connection.set_connected(true);

    let (candidate, mut candidate_receiver) = connections
      .prepare(server_id.clone(), test_args("server-1"))
      .await;
    candidate.set_error(anyhow!("auth failed")).await;

    let current = connections.get(&server_id).await.unwrap();
    assert!(Arc::ptr_eq(&current, &old_connection));
    assert!(!old_connection.cancel.is_cancelled());
    assert!(old_connection.connected());

    current
      .sender
      .send_message(LoginMessage::Success)
      .await
      .unwrap();

    timeout(Duration::from_millis(100), old_receiver.recv())
      .await
      .expect(
        "old connection should remain routed when auth fails before publish",
      )
      .unwrap();

    assert!(
      timeout(Duration::from_millis(100), candidate_receiver.recv())
        .await
        .is_err(),
      "failed unpublished replacement should not receive routed messages"
    );
  }

  #[tokio::test]
  async fn prepare_then_publish_routes_new_messages_to_replacement() {
    let connections = PeripheryConnections::default();
    let server_id = String::from("server-1");

    let (old_connection, mut old_receiver) = connections
      .prepare(server_id.clone(), test_args("server-1"))
      .await;
    connections
      .publish(server_id.clone(), old_connection.clone())
      .await;
    old_connection.set_connected(true);

    let (replacement, mut replacement_receiver) = connections
      .prepare(server_id.clone(), test_args("server-1"))
      .await;
    connections
      .publish(server_id.clone(), replacement.clone())
      .await;

    assert!(old_connection.cancel.is_cancelled());
    assert!(!old_connection.connected());

    let current = connections.get(&server_id).await.unwrap();
    assert!(Arc::ptr_eq(&current, &replacement));

    current
      .sender
      .send_message(LoginMessage::Success)
      .await
      .unwrap();

    timeout(Duration::from_millis(100), replacement_receiver.recv())
      .await
      .expect("replacement should receive routed message")
      .unwrap();

    assert!(
      timeout(Duration::from_millis(100), old_receiver.recv())
        .await
        .is_err(),
      "old connection should not receive newly routed messages after replacement publish"
    );
  }
}

use std::{
  sync::{Arc, LazyLock},
  time::Duration,
};

use anyhow::{Context, anyhow};
use database::mungos::mongodb::bson::doc;
use komodo_client::entities::{komodo_timestamp, user::User};
use mogh_auth_server::RequestAuthentication;
use tokio::{
  sync::{OwnedSemaphorePermit, Semaphore},
  time::timeout,
};

use crate::{
  auth::JWT_PROVIDER, helpers::query::get_user, state::db_client,
};

pub async fn extract_user_from_auth(
  auth: RequestAuthentication,
  require_user_enabled: bool,
) -> anyhow::Result<User> {
  let user_id = match auth {
    RequestAuthentication::UserId(user_id) => user_id,
    RequestAuthentication::KeyAndSecret { key, secret } => {
      auth_api_key_get_user_id(&key, &secret).await?
    }
    RequestAuthentication::PublicKey(_) => todo!(),
  };
  if require_user_enabled {
    check_enabled(&user_id).await
  } else {
    get_user(&user_id).await
  }
}

pub async fn auth_jwt_check_enabled(
  jwt: &str,
) -> anyhow::Result<User> {
  let user_id = JWT_PROVIDER.decode_sub(jwt)?;
  check_enabled(&user_id).await
}

pub async fn auth_api_key_check_enabled(
  key: &str,
  secret: &str,
) -> anyhow::Result<User> {
  let user_id = auth_api_key_get_user_id(key, secret).await?;
  check_enabled(&user_id).await
}

/// Api Key Clock skew tolerance in milliseconds (5 minutes for Api Keys)
const API_KEY_CLOCK_SKEW_TOLERANCE_MS: i64 = 5 * 60 * 1000;
const BCRYPT_MAX_CONCURRENT: usize = 8;
const BCRYPT_MAX_QUEUED: usize = 64;
const BCRYPT_ADMISSION_TIMEOUT: Duration = Duration::from_secs(2);

static BCRYPT_VERIFIER: LazyLock<Arc<BcryptVerifier>> =
  LazyLock::new(|| {
    Arc::new(BcryptVerifier::new(
      BCRYPT_MAX_CONCURRENT,
      BCRYPT_MAX_QUEUED,
      BCRYPT_ADMISSION_TIMEOUT,
    ))
  });

#[derive(Clone)]
struct BcryptVerifier {
  queue: Arc<Semaphore>,
  execution: Arc<Semaphore>,
  admission_timeout: Duration,
}

impl BcryptVerifier {
  fn new(
    max_concurrent: usize,
    max_queued: usize,
    admission_timeout: Duration,
  ) -> Self {
    Self {
      queue: Arc::new(Semaphore::new(max_queued)),
      execution: Arc::new(Semaphore::new(max_concurrent)),
      admission_timeout,
    }
  }

  async fn acquire_queue_permit(
    &self,
  ) -> anyhow::Result<OwnedSemaphorePermit> {
    timeout(
      self.admission_timeout,
      self.queue.clone().acquire_owned(),
    )
    .await
    .map_err(|_| anyhow!("Invalid user credentials"))?
    .map_err(|_| anyhow!("Invalid user credentials"))
  }

  async fn acquire_execution_permit(
    &self,
  ) -> anyhow::Result<OwnedSemaphorePermit> {
    self
      .execution
      .clone()
      .acquire_owned()
      .await
      .map_err(|_| anyhow!("Invalid user credentials"))
  }
}

async fn verify_api_secret(
  secret: &str,
  hashed_secret: &str,
) -> anyhow::Result<bool> {
  verify_api_secret_with(
    BCRYPT_VERIFIER.clone(),
    secret.to_string(),
    hashed_secret.to_string(),
    |secret, hashed_secret| {
      bcrypt::verify(&secret, &hashed_secret)
        .map_err(|_| anyhow!("Invalid user credentials"))
    },
  )
  .await
}

async fn verify_api_secret_with<F>(
  verifier: Arc<BcryptVerifier>,
  secret: String,
  hashed_secret: String,
  verify: F,
) -> anyhow::Result<bool>
where
  F: FnOnce(String, String) -> anyhow::Result<bool> + Send + 'static,
{
  let queue_permit = verifier.acquire_queue_permit().await?;
  let execution_permit = verifier.acquire_execution_permit().await?;
  drop(queue_permit);

  tokio::task::spawn_blocking(move || {
    let _execution_permit = execution_permit;
    verify(secret, hashed_secret)
  })
  .await
  .context("Failed to join bcrypt verifier task")?
}

pub async fn auth_api_key_get_user_id(
  key: &str,
  secret: &str,
) -> anyhow::Result<String> {
  let key = db_client()
    .api_keys
    .find_one(doc! { "key": key })
    .await
    .context("Failed to query db")?
    .context("Invalid user credentials")?;
  // Apply clock skew tolerance.
  // Token is invalid if expiration is less than (now - tolerance)
  if key.expires != 0
    && key.expires
      < komodo_timestamp()
        .saturating_sub(API_KEY_CLOCK_SKEW_TOLERANCE_MS)
  {
    return Err(anyhow!("Invalid user credentials"));
  }
  if verify_api_secret(secret, &key.secret).await? {
    // secret matches
    Ok(key.user_id)
  } else {
    // secret mismatch
    Err(anyhow!("Invalid user credentials"))
  }
}

async fn check_enabled(user_id: &str) -> anyhow::Result<User> {
  let user = get_user(user_id).await?;
  if user.enabled {
    Ok(user)
  } else {
    Err(anyhow!("Invalid user credentials"))
  }
}

#[cfg(test)]
mod tests {
  use std::{
    sync::{
      Arc, Condvar, Mutex,
      atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
  };

  use anyhow::anyhow;
  use tokio::time::timeout;

  use super::{
    BCRYPT_MAX_CONCURRENT, BCRYPT_MAX_QUEUED, BcryptVerifier,
    verify_api_secret_with,
  };

  fn update_max(max_active: &AtomicUsize, current: usize) {
    let mut observed = max_active.load(Ordering::SeqCst);
    while current > observed {
      match max_active.compare_exchange(
        observed,
        current,
        Ordering::SeqCst,
        Ordering::SeqCst,
      ) {
        Ok(_) => break,
        Err(next) => observed = next,
      }
    }
  }

  fn wait_on_gate(gate: &Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cvar) = &**gate;
    let mut released = lock.lock().unwrap();
    while !*released {
      released = cvar.wait(released).unwrap();
    }
  }

  fn release_gate(gate: &Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cvar) = &**gate;
    let mut released = lock.lock().unwrap();
    *released = true;
    cvar.notify_all();
  }

  async fn wait_until(
    mut predicate: impl FnMut() -> bool,
    timeout_duration: Duration,
  ) {
    timeout(timeout_duration, async {
      while !predicate() {
        tokio::time::sleep(Duration::from_millis(10)).await;
      }
    })
    .await
    .expect("condition was not met before timeout");
  }

  #[tokio::test(flavor = "current_thread")]
  async fn api_secret_verification_runs_off_runtime_thread() {
    let verifier =
      Arc::new(BcryptVerifier::new(1, 1, Duration::from_millis(250)));
    let runtime_thread = thread::current().id();
    let (thread_tx, thread_rx) = std::sync::mpsc::channel();

    let verified = verify_api_secret_with(
      verifier.clone(),
      "secret".to_string(),
      "hash".to_string(),
      move |_, _| {
        thread_tx.send(thread::current().id()).unwrap();
        thread::sleep(Duration::from_millis(25));
        Ok(true)
      },
    )
    .await
    .unwrap();

    assert!(verified);
    assert_ne!(
      thread_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
      runtime_thread
    );
  }

  #[tokio::test(flavor = "current_thread")]
  async fn api_secret_verification_caps_32_way_concurrency_at_eight()
  {
    let verifier = Arc::new(BcryptVerifier::new(
      BCRYPT_MAX_CONCURRENT,
      BCRYPT_MAX_QUEUED,
      Duration::from_secs(1),
    ));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    for _ in 0..32 {
      let gate = gate.clone();
      let active = active.clone();
      let max_active = max_active.clone();
      handles.push(tokio::spawn(verify_api_secret_with(
        verifier.clone(),
        "secret".to_string(),
        "hash".to_string(),
        move |_, _| {
          let current = active.fetch_add(1, Ordering::SeqCst) + 1;
          update_max(&max_active, current);
          wait_on_gate(&gate);
          active.fetch_sub(1, Ordering::SeqCst);
          Ok(true)
        },
      )));
    }

    wait_until(
      || max_active.load(Ordering::SeqCst) == BCRYPT_MAX_CONCURRENT,
      Duration::from_secs(1),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
      max_active.load(Ordering::SeqCst),
      BCRYPT_MAX_CONCURRENT
    );

    release_gate(&gate);
    for handle in handles {
      assert!(handle.await.unwrap().unwrap());
    }
  }

  #[tokio::test(flavor = "current_thread")]
  async fn api_secret_verification_rejects_waiter_65_when_queue_is_full()
   {
    let verifier = Arc::new(BcryptVerifier::new(
      BCRYPT_MAX_CONCURRENT,
      BCRYPT_MAX_QUEUED,
      Duration::from_millis(50),
    ));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let mut running = Vec::new();
    let mut queued = Vec::new();

    for _ in 0..BCRYPT_MAX_CONCURRENT {
      let gate = gate.clone();
      running.push(tokio::spawn(verify_api_secret_with(
        verifier.clone(),
        "secret".to_string(),
        "hash".to_string(),
        move |_, _| {
          wait_on_gate(&gate);
          Ok(true)
        },
      )));
    }

    for _ in 0..BCRYPT_MAX_QUEUED {
      queued.push(tokio::spawn(verify_api_secret_with(
        verifier.clone(),
        "secret".to_string(),
        "hash".to_string(),
        |_, _| Ok(true),
      )));
    }

    wait_until(
      || verifier.queue.available_permits() == 0,
      Duration::from_secs(1),
    )
    .await;

    let waiter_65 = verify_api_secret_with(
      verifier.clone(),
      "secret".to_string(),
      "hash".to_string(),
      |_, _| Ok(true),
    )
    .await;

    assert!(waiter_65.is_err());

    release_gate(&gate);
    for handle in running {
      assert!(handle.await.unwrap().unwrap());
    }
    for handle in queued {
      assert!(handle.await.unwrap().unwrap());
    }
  }

  #[tokio::test(flavor = "current_thread")]
  async fn cancelled_waiter_releases_queue_capacity() {
    let verifier =
      Arc::new(BcryptVerifier::new(1, 1, Duration::from_millis(250)));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));

    let running = {
      let gate = gate.clone();
      tokio::spawn(verify_api_secret_with(
        verifier.clone(),
        "secret".to_string(),
        "hash".to_string(),
        move |_, _| {
          wait_on_gate(&gate);
          Ok(true)
        },
      ))
    };

    let waiter = tokio::spawn(verify_api_secret_with(
      verifier.clone(),
      "secret".to_string(),
      "hash".to_string(),
      |_, _| Ok(true),
    ));

    wait_until(
      || verifier.queue.available_permits() == 0,
      Duration::from_secs(1),
    )
    .await;

    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());

    wait_until(
      || verifier.queue.available_permits() == 1,
      Duration::from_secs(1),
    )
    .await;

    let replacement = tokio::spawn(verify_api_secret_with(
      verifier.clone(),
      "secret".to_string(),
      "hash".to_string(),
      |_, _| Ok(true),
    ));

    wait_until(
      || verifier.queue.available_permits() == 0,
      Duration::from_secs(1),
    )
    .await;

    release_gate(&gate);
    assert!(running.await.unwrap().unwrap());
    assert!(replacement.await.unwrap().unwrap());
  }

  #[tokio::test(flavor = "current_thread")]
  async fn invalid_bcrypt_hash_maps_to_invalid_user_credentials() {
    let verifier =
      Arc::new(BcryptVerifier::new(1, 1, Duration::from_millis(250)));

    let error = verify_api_secret_with(
      verifier.clone(),
      "secret".to_string(),
      "not-a-bcrypt-hash".to_string(),
      |secret, hash| {
        bcrypt::verify(&secret, &hash)
          .map_err(|_| anyhow!("Invalid user credentials"))
      },
    )
    .await
    .unwrap_err();

    assert_eq!(error.to_string(), "Invalid user credentials");
  }
}

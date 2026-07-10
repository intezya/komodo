//! Pull-based GitOps reconciliation.
//!
//! The controller only owns explicit opt-ins. It delegates mutations to the
//! existing execution resolvers so normal action-state guards remain active.

use std::{
  collections::{HashMap, HashSet},
  sync::OnceLock,
  time::{Duration, Instant},
};

use async_timing_util::{Timelength, get_timelength_in_ms};
use database::mungos::find::find_collect;
use komodo_client::entities::{
  repo::Repo, stack::Stack, sync::ResourceSync,
};
use tokio::sync::Mutex;

use crate::{config::core_config, state::db_client};

mod source;
mod stack;
mod sync;

use source::{
  GitSourceConsumer, GitSourceKey, fetch_source,
  resolve_stack_source, resolve_sync_source,
};

fn cycle_lock() -> &'static Mutex<()> {
  static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
  LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn spawn_gitops_controller() {
  let interval: Timelength = core_config()
    .resource_poll_interval
    .try_into()
    .expect("invalid resource poll interval");
  tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_millis(
      get_timelength_in_ms(interval) as u64,
    ));
    loop {
      interval.tick().await;
      let Ok(_guard) = cycle_lock().try_lock() else {
        warn!("skipping overlapping GitOps controller cycle");
        continue;
      };
      reconcile_cycle().await;
    }
  });
}

async fn reconcile_cycle() {
  let started = Instant::now();
  let Ok(repos) = find_collect(&db_client().repos, None, None).await
  else {
    warn!("failed to load Repos for GitOps controller");
    return;
  };
  let repos = repos
    .into_iter()
    .map(|repo: Repo| (repo.name.clone(), repo))
    .collect::<HashMap<_, _>>();
  let Ok(syncs) =
    find_collect(&db_client().resource_syncs, None, None).await
  else {
    warn!("failed to load Resource Syncs for GitOps controller");
    return;
  };
  let Ok(stacks) =
    find_collect(&db_client().stacks, None, None).await
  else {
    warn!("failed to load Stacks for GitOps controller");
    return;
  };
  let mut groups =
    HashMap::<GitSourceKey, Vec<GitSourceConsumer>>::new();
  let mut sync_sources = Vec::new();
  for sync in syncs.into_iter().filter(is_safe_opted_in_sync) {
    let Some(source) = resolve_sync_source(&sync, &repos) else {
      continue;
    };
    groups
      .entry(source.key.clone())
      .or_default()
      .push(GitSourceConsumer::ResourceSync(sync.id.clone()));
    sync_sources.push((sync, source.key));
  }
  let preexisting_stack_ids = stacks
    .iter()
    .filter(|stack| is_opted_in_stack(stack))
    .map(|stack| stack.id.clone())
    .collect::<HashSet<_>>();
  for stack in stacks.into_iter().filter(is_opted_in_stack) {
    let Some(source) = resolve_stack_source(&stack, &repos) else {
      continue;
    };
    groups
      .entry(source.key)
      .or_default()
      .push(GitSourceConsumer::Stack(stack.id));
  }
  let mut snapshots = HashMap::with_capacity(groups.len());
  for key in groups.keys() {
    snapshots.insert(key.clone(), fetch_source(key).await);
  }
  let source_count = snapshots.len();
  reconcile_syncs(sync_sources, &snapshots).await;
  reconcile_stacks(preexisting_stack_ids, &repos, &snapshots).await;
  info!(
    sources = source_count,
    elapsed_ms = started.elapsed().as_millis(),
    "completed GitOps controller cycle"
  );
}

async fn reconcile_syncs(
  sync_sources: Vec<(ResourceSync, GitSourceKey)>,
  snapshots: &HashMap<GitSourceKey, source::GitSourceSnapshot>,
) {
  for (sync, source) in sync_sources {
    let Some(snapshot) = snapshots.get(&source) else {
      continue;
    };
    let Some(checkout_root) = snapshot.checkout_root.as_deref()
    else {
      warn!(
        sync = sync.name,
        error = snapshot
          .error
          .as_deref()
          .unwrap_or("unknown source failure"),
        "GitOps source fetch failed"
      );
      continue;
    };
    let remote = crate::sync::remote::read_prepared_repo_resources(
      &sync,
      checkout_root,
      snapshot.hash.clone(),
      snapshot.message.clone(),
      Vec::new(),
    );
    if let Err(error) =
      sync::reconcile_prepared_sync(sync.clone(), remote).await
    {
      warn!(sync = sync.name, error = %error, "GitOps sync failed");
    }
  }
}

fn is_safe_opted_in_sync(sync: &ResourceSync) -> bool {
  sync.config.auto_apply_updates
    && !sync.config.files_on_host
    && (!sync.config.repo.is_empty()
      || !sync.config.linked_repo.is_empty())
    && sync.config.commit.is_empty()
}

async fn reconcile_stacks(
  preexisting_stack_ids: HashSet<String>,
  repos: &HashMap<String, Repo>,
  snapshots: &HashMap<GitSourceKey, source::GitSourceSnapshot>,
) {
  let Ok(stacks) =
    find_collect(&db_client().stacks, None, None).await
  else {
    warn!("failed to load Stacks for GitOps controller");
    return;
  };
  for stack in stacks.into_iter().filter(|stack| {
    preexisting_stack_ids.contains(&stack.id)
      && is_opted_in_stack(stack)
  }) {
    let Some(source) = resolve_stack_source(&stack, repos) else {
      continue;
    };
    let Some(snapshot) = snapshots.get(&source.key) else {
      continue;
    };
    let Some(checkout_root) = snapshot.checkout_root.as_deref()
    else {
      continue;
    };
    let remote =
      crate::stack::remote::read_prepared_repo_compose_contents(
        &stack,
        checkout_root,
        snapshot.hash.clone(),
        snapshot.message.clone(),
        None,
      );
    if let Err(error) =
      stack::reconcile_prepared_stack(stack.clone(), remote).await
    {
      warn!(stack = stack.name, error = %error, "GitOps stack reconciliation failed");
    }
  }
}

fn is_opted_in_stack(stack: &Stack) -> bool {
  stack.config.auto_deploy_git_updates
    && !stack.config.files_on_host
    && (!stack.config.repo.is_empty()
      || !stack.config.linked_repo.is_empty())
    && stack.config.commit.is_empty()
}

use std::{collections::HashMap, path::PathBuf};

use komodo_client::entities::{
  DefaultRepoFolder, RepoExecutionArgs, repo::Repo, stack::Stack,
  sync::ResourceSync,
};

use crate::{config::core_config, helpers::git_token};

/// Identity of a moving Git branch fetched once per controller cycle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct GitSourceKey {
  pub provider: String,
  pub account: String,
  pub repo: String,
  pub branch: String,
}

/// A resource source after resolving an optional linked Repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectiveGitSource {
  pub key: GitSourceKey,
}

/// A controller consumer of a shared source snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum GitSourceConsumer {
  Stack(String),
  ResourceSync(String),
}

/// Sanitized result of a single source fetch. It never contains credentials.
#[derive(Debug, Clone, Default)]
pub(super) struct GitSourceSnapshot {
  pub checkout_root: Option<PathBuf>,
  pub hash: Option<String>,
  pub message: Option<String>,
  pub error: Option<String>,
}

impl GitSourceSnapshot {
  fn failed() -> Self {
    Self {
      error: Some("failed to fetch Git source".into()),
      ..Default::default()
    }
  }
}

#[cfg(test)]
pub(super) fn fetch_source_groups<F>(
  groups: impl IntoIterator<Item = (GitSourceKey, Vec<GitSourceConsumer>)>,
  mut fetch: F,
) -> HashMap<GitSourceKey, GitSourceSnapshot>
where
  F: FnMut(&GitSourceKey) -> anyhow::Result<GitSourceSnapshot>,
{
  groups
    .into_iter()
    .map(|(key, _)| {
      let snapshot =
        fetch(&key).unwrap_or_else(|_| GitSourceSnapshot::failed());
      (key, snapshot)
    })
    .collect()
}

pub(super) async fn fetch_source(
  key: &GitSourceKey,
) -> GitSourceSnapshot {
  let mut args = RepoExecutionArgs {
    name: key.repo.clone(),
    provider: key.provider.clone(),
    https: true,
    account: (!key.account.is_empty()).then(|| key.account.clone()),
    repo: Some(key.repo.clone()),
    branch: key.branch.clone(),
    commit: None,
    destination: None,
    default_folder: DefaultRepoFolder::NotApplicable,
  };
  let token = match args.account.as_deref() {
    Some(account) => {
      match git_token(&args.provider, account, |https| {
        args.https = https;
      })
      .await
      {
        Ok(token) => token,
        Err(_) => return GitSourceSnapshot::failed(),
      }
    }
    None => None,
  };
  let checkout_root =
    match args.unique_path(&core_config().repo_directory) {
      Ok(path) => path,
      Err(_) => return GitSourceSnapshot::failed(),
    };
  args.destination = Some(checkout_root.display().to_string());
  match git::pull_or_clone(args, &core_config().repo_directory, token)
    .await
  {
    Ok((result, _)) if result.logs.iter().all(|log| log.success) => {
      GitSourceSnapshot {
        checkout_root: Some(checkout_root),
        hash: result.commit_hash,
        message: result.commit_message,
        error: None,
      }
    }
    _ => GitSourceSnapshot::failed(),
  }
}

pub(super) fn resolve_stack_source(
  stack: &Stack,
  repos: &HashMap<String, Repo>,
) -> Option<EffectiveGitSource> {
  if stack.config.files_on_host || !stack.config.commit.is_empty() {
    return None;
  }
  let source = if stack.config.linked_repo.is_empty() {
    SourceFields::from_stack(stack)
  } else {
    SourceFields::from_repo(repos.get(&stack.config.linked_repo)?)
  };
  source.into_effective()
}

pub(super) fn resolve_sync_source(
  sync: &ResourceSync,
  repos: &HashMap<String, Repo>,
) -> Option<EffectiveGitSource> {
  if sync.config.files_on_host
    || !sync.config.commit.is_empty()
    || (!sync.config.linked_repo.is_empty()
      && !repos.contains_key(&sync.config.linked_repo))
  {
    return None;
  }
  let source = if sync.config.linked_repo.is_empty() {
    SourceFields::from_sync(sync)
  } else {
    SourceFields::from_repo(repos.get(&sync.config.linked_repo)?)
  };
  source.into_effective()
}

struct SourceFields {
  provider: String,
  account: String,
  repo: String,
  branch: String,
  commit: String,
}

impl SourceFields {
  fn from_stack(stack: &Stack) -> Self {
    Self {
      provider: stack.config.git_provider.clone(),
      account: stack.config.git_account.clone(),
      repo: stack.config.repo.clone(),
      branch: stack.config.branch.clone(),
      commit: stack.config.commit.clone(),
    }
  }

  fn from_sync(sync: &ResourceSync) -> Self {
    Self {
      provider: sync.config.git_provider.clone(),
      account: sync.config.git_account.clone(),
      repo: sync.config.repo.clone(),
      branch: sync.config.branch.clone(),
      commit: sync.config.commit.clone(),
    }
  }

  fn from_repo(repo: &Repo) -> Self {
    Self {
      provider: repo.config.git_provider.clone(),
      account: repo.config.git_account.clone(),
      repo: repo.config.repo.clone(),
      branch: repo.config.branch.clone(),
      commit: repo.config.commit.clone(),
    }
  }

  fn into_effective(self) -> Option<EffectiveGitSource> {
    if self.repo.is_empty() || !self.commit.is_empty() {
      return None;
    }
    Some(EffectiveGitSource {
      key: GitSourceKey {
        provider: self.provider,
        account: self.account,
        repo: self.repo,
        branch: self.branch,
      },
    })
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use komodo_client::entities::{
    repo::Repo, stack::Stack, sync::ResourceSync,
  };

  use super::*;

  #[test]
  fn direct_stack_and_sync_share_a_source_key() {
    let mut stack = Stack::default();
    stack.config.repo = "acme/gitops".into();
    stack.config.git_account = "automation".into();

    let mut sync = ResourceSync::default();
    sync.config.repo = "acme/gitops".into();
    sync.config.git_account = "automation".into();

    let repos = HashMap::<String, Repo>::new();
    let stack_source = resolve_stack_source(&stack, &repos)
      .expect("stack should be eligible");
    let sync_source = resolve_sync_source(&sync, &repos)
      .expect("sync should be eligible");

    assert_eq!(stack_source.key, sync_source.key);
  }

  #[test]
  fn fixed_commit_and_non_git_sources_are_ineligible() {
    let repos = HashMap::<String, Repo>::new();
    let mut stack = Stack::default();
    stack.config.repo = "acme/gitops".into();
    stack.config.commit = "deadbeef".into();
    assert!(resolve_stack_source(&stack, &repos).is_none());

    let mut sync = ResourceSync::default();
    sync.config.files_on_host = true;
    assert!(resolve_sync_source(&sync, &repos).is_none());
  }

  #[test]
  fn linked_repo_defines_the_effective_source() {
    let mut repo = Repo::default();
    repo.name = "shared-source".into();
    repo.config.repo = "acme/gitops".into();
    repo.config.git_account = "automation".into();
    repo.config.branch = "production".into();
    let repos = HashMap::from([(repo.name.clone(), repo)]);

    let mut stack = Stack::default();
    stack.config.linked_repo = "shared-source".into();

    let source = resolve_stack_source(&stack, &repos)
      .expect("linked repo should resolve");
    assert_eq!(source.key.repo, "acme/gitops");
    assert_eq!(source.key.branch, "production");
  }

  #[test]
  fn fetches_each_source_group_once() {
    let key = GitSourceKey {
      provider: "github.com".into(),
      account: "automation".into(),
      repo: "acme/gitops".into(),
      branch: "main".into(),
    };
    let mut calls = 0;
    let snapshots = fetch_source_groups(
      [(
        key.clone(),
        vec![
          GitSourceConsumer::Stack("stack-id".into()),
          GitSourceConsumer::ResourceSync("sync-id".into()),
        ],
      )],
      |_| {
        calls += 1;
        Ok(GitSourceSnapshot {
          checkout_root: Some("/tmp/gitops".into()),
          ..Default::default()
        })
      },
    );

    assert_eq!(calls, 1);
    assert!(snapshots[&key].checkout_root.is_some());
  }
}

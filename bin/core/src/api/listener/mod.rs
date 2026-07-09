use std::sync::Arc;

use anyhow::anyhow;
use axum::{Router, http::HeaderMap};
use komodo_client::entities::resource::Resource;
use mogh_cache::CloneCache;
use tokio::sync::Mutex;

use crate::resource::KomodoResource;

mod integrations;
mod resources;
mod router;

use integrations::*;

pub fn router() -> Router {
  Router::new()
    .nest("/github", router::router::<github::Github>())
    .nest("/gitlab", router::router::<gitlab::Gitlab>())
}

type ListenerLockCache = CloneCache<String, Arc<Mutex<()>>>;

fn normalize_branch(branch: &str) -> &str {
  let branch = branch.trim();
  branch.strip_prefix("refs/heads/").unwrap_or(branch)
}

/// Implemented for all resources which can recieve webhook.
trait CustomSecret: KomodoResource {
  fn custom_secret(
    resource: &Resource<Self::Config, Self::Info>,
  ) -> &str;
}

/// Implemented on the integration struct, eg [integrations::github::Github]
trait VerifySecret {
  fn verify_secret(
    headers: &HeaderMap,
    body: &str,
    custom_secret: &str,
  ) -> anyhow::Result<()>;
}

/// Implemented on the integration struct, eg [integrations::github::Github]
trait ExtractBranch {
  fn extract_branch(body: &str) -> anyhow::Result<String>;
  fn verify_branch(body: &str, expected: &str) -> anyhow::Result<()> {
    let branch = Self::extract_branch(body)?;
    let branch = normalize_branch(&branch);
    let expected = normalize_branch(expected);
    if branch == expected {
      Ok(())
    } else {
      Err(anyhow!(
        "request branch '{branch}' does not match expected '{expected}'"
      ))
    }
  }
}

/// For Procedures and Actions, incoming webhook
/// can be triggered by any branch by using `__ANY__`
/// as the branch in the webhook URL.
const ANY_BRANCH: &str = "__ANY__";

#[cfg(test)]
mod tests {
  use super::{ExtractBranch, integrations::github::Github};

  #[test]
  fn github_ref_heads_main_matches_main() {
    let body = r#"{"ref":"refs/heads/main"}"#;

    let result =
      <Github as ExtractBranch>::verify_branch(body, "main");

    assert!(result.is_ok());
  }

  #[test]
  fn plain_main_matches_main() {
    let body = r#"{"ref":"main"}"#;

    let result =
      <Github as ExtractBranch>::verify_branch(body, "main");

    assert!(result.is_ok());
  }

  #[test]
  fn feature_branch_does_not_match_main() {
    let body = r#"{"ref":"feature/test"}"#;

    let error =
      <Github as ExtractBranch>::verify_branch(body, "main")
        .unwrap_err()
        .to_string();

    assert_eq!(
      error,
      "request branch 'feature/test' does not match expected 'main'"
    );
  }

  #[test]
  fn expected_refs_heads_main_matches_payload_main() {
    let body = r#"{"ref":"main"}"#;

    let result = <Github as ExtractBranch>::verify_branch(
      body,
      "refs/heads/main",
    );

    assert!(result.is_ok());
  }
}

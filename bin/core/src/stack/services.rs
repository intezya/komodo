use std::collections::HashMap;

use anyhow::Context;
use komodo_client::entities::{
  ImageDigest,
  stack::{ComposeFile, ComposeService, Stack, StackServiceNames},
};

pub fn extract_services_from_stack(
  stack: &Stack,
) -> Vec<StackServiceNames> {
  if let Some(mut services) = stack.info.deployed_services.clone() {
    for service in services.iter_mut().filter(|s| s.image.is_empty())
    {
      service.image = stack
        .info
        .latest_services
        .iter()
        .find(|s| s.service_name == service.service_name)
        .map(|s| s.image.clone())
        .unwrap_or_default();
    }
    services
  } else {
    stack.info.latest_services.clone()
  }
}

pub fn extract_services_into_res(
  project_name: &str,
  compose_contents: &str,
  service_image_digests: &HashMap<String, ImageDigest>,
  res: &mut Vec<StackServiceNames>,
) -> anyhow::Result<()> {
  let compose =
    serde_yaml_ng::from_str::<ComposeFile>(compose_contents)
      .context(
        "failed to parse service names from compose contents",
      )?;

  for (
    service_name,
    ComposeService {
      container_name,
      image,
      deploy,
      ..
    },
  ) in compose.services
  {
    if let Some(existing) =
      res.iter_mut().find(|s| s.service_name == service_name)
    {
      // Override any defined fields
      if let Some(container_name) = container_name {
        existing.container_name = container_name;
      }
      if let Some(image) = image {
        existing.image = image;
      }
      existing.desired_replicas =
        deploy.and_then(|deploy| deploy.replicas).unwrap_or(1);
    } else {
      res.push(StackServiceNames {
        desired_replicas: deploy
          .and_then(|deploy| deploy.replicas)
          .unwrap_or(1),
        container_name: container_name.unwrap_or_else(|| {
          format!("{project_name}-{service_name}")
        }),
        image_digest: service_image_digests
          .get(&service_name)
          .cloned(),
        image: image.unwrap_or_default(),
        service_name,
      });
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn replicated_compose_service_is_one_logical_service() {
    let mut services = Vec::new();

    extract_services_into_res(
      "app",
      r#"
services:
  web:
    image: example/web:latest
    deploy:
      replicas: 3
"#,
      &HashMap::new(),
      &mut services,
    )
    .expect("compose services should parse");

    assert_eq!(services.len(), 1);
    assert_eq!(services[0].service_name, "web");
    assert_eq!(services[0].desired_replicas, 3);
  }
}

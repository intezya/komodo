use std::{
  io::Error,
  os::unix::process::ExitStatusExt,
  process::{ExitStatus, Stdio},
};

use anyhow::{Context, anyhow};
use bollard::Docker;
use command::{CommandOutput, run_komodo_standard_command};
use komodo_client::entities::{
  TerminationSignal,
  docker::{task::*, *},
  update::Log,
};
use tokio::{io::AsyncWriteExt, process::Command};

pub mod compose;
pub mod config;
pub mod image;
pub mod secret;
pub mod stack;
pub mod stats;

mod container;
mod network;
mod node;
mod service;
mod swarm;
mod task;
mod volume;

pub struct DockerClient {
  docker: Docker,
}

impl DockerClient {
  pub fn connect() -> anyhow::Result<DockerClient> {
    let docker = Docker::connect_with_defaults()
      .context("Failed to connect to docker api. Docker monitoring won't work and will return empty results.")?;
    Ok(DockerClient { docker })
  }
}

/// Returns whether login was actually performed.
#[instrument("DockerLogin", skip(registry_token))]
pub async fn docker_login(
  domain: &str,
  account: &str,
  // For local token override from core.
  registry_token: Option<&str>,
) -> anyhow::Result<bool> {
  if domain.is_empty() || account.is_empty() {
    return Ok(false);
  }

  let registry_token = match registry_token {
    Some(token) => token,
    None => crate::helpers::registry_token(domain, account)?,
  };

  let output = sanitize_docker_login_output(
    run_docker_login_command(domain, account, registry_token).await,
    registry_token,
  );

  if output.success() {
    return Ok(true);
  }

  let mut e = anyhow!("End of trace");
  for line in output
    .stderr
    .split('\n')
    .filter(|line| !line.is_empty())
    .rev()
  {
    e = e.context(line.to_string());
  }
  for line in output
    .stdout
    .split('\n')
    .filter(|line| !line.is_empty())
    .rev()
  {
    e = e.context(line.to_string());
  }
  Err(e.context(format!("Registry {domain} login error")))
}

fn docker_login_args(domain: &str, account: &str) -> Vec<String> {
  vec![
    "login".into(),
    domain.into(),
    "--username".into(),
    account.into(),
    "--password-stdin".into(),
  ]
}

fn docker_login_error_output(
  stderr: impl Into<String>,
) -> CommandOutput {
  CommandOutput {
    status: ExitStatus::from_raw(1),
    stdout: String::new(),
    stderr: stderr.into(),
  }
}

fn sanitize_docker_login_output(
  output: CommandOutput,
  registry_token: &str,
) -> CommandOutput {
  if registry_token.is_empty() {
    return output;
  }

  CommandOutput {
    stdout: redact_docker_login_token_fragments(
      &output.stdout,
      registry_token,
    ),
    stderr: redact_docker_login_token_fragments(
      &output.stderr,
      registry_token,
    ),
    ..output
  }
}

fn redact_docker_login_token_fragments(
  text: &str,
  registry_token: &str,
) -> String {
  let text_chars = text.chars().collect::<Vec<_>>();
  let token_chars = registry_token.chars().collect::<Vec<_>>();
  let min_match_len = token_chars.len().min(4);

  if min_match_len == 0 {
    return text.to_string();
  }

  let mut redacted = String::with_capacity(text.len());
  let mut index = 0;

  while index < text_chars.len() {
    let match_len =
      longest_token_fragment_match(&text_chars[index..], &token_chars);

    if match_len >= min_match_len
      || is_delimited_short_token_fragment(
        &text_chars,
        index,
        match_len,
      )
    {
      redacted.push_str("[REDACTED]");
      index += match_len;
      continue;
    }

    redacted.push(text_chars[index]);
    index += 1;
  }

  redacted
}

fn is_delimited_short_token_fragment(
  text: &[char],
  start: usize,
  match_len: usize,
) -> bool {
  if match_len == 0 {
    return false;
  }

  let before = start
    .checked_sub(1)
    .and_then(|index| text.get(index))
    .copied();
  let after = text.get(start + match_len).copied();

  is_non_alphanumeric_boundary(before)
    && is_non_alphanumeric_boundary(after)
}

fn is_non_alphanumeric_boundary(character: Option<char>) -> bool {
  match character {
    Some(character) => !character.is_alphanumeric(),
    None => true,
  }
}

fn longest_token_fragment_match(
  text: &[char],
  registry_token: &[char],
) -> usize {
  let mut best = 0;

  for token_start in 0..registry_token.len() {
    let mut len = 0;
    while len < text.len()
      && token_start + len < registry_token.len()
      && text[len] == registry_token[token_start + len]
    {
      len += 1;
    }
    best = best.max(len);
  }

  best
}

async fn run_docker_login_command(
  domain: &str,
  account: &str,
  registry_token: &str,
) -> CommandOutput {
  let mut child = match Command::new("docker")
    .args(docker_login_args(domain, account))
    .kill_on_drop(true)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
  {
    Ok(child) => child,
    Err(error) => {
      return docker_login_error_output(format!(
        "Failed to start docker login command: {error}"
      ));
    }
  };

  let Some(mut stdin) = child.stdin.take() else {
    return docker_login_error_output(
      Error::other("Failed to open docker login stdin").to_string(),
    );
  };

  if let Err(error) = stdin.write_all(registry_token.as_bytes()).await
  {
    return docker_login_error_output(format!(
      "Failed to write docker login token to stdin: {error}"
    ));
  }

  drop(stdin);

  match child.wait_with_output().await {
    Ok(output) => CommandOutput::from(Ok(output)),
    Err(error) => docker_login_error_output(format!(
      "Failed to wait for docker login command: {error}"
    )),
  }
}

#[instrument("PullImage")]
pub async fn pull_image(image: &str) -> Log {
  let command = format!("docker pull {image}");
  run_komodo_standard_command("Docker Pull", None, command).await
}

pub fn stop_container_command(
  container_name: &str,
  signal: Option<TerminationSignal>,
  time: Option<i32>,
) -> String {
  let signal = signal
    .map(|signal| format!(" --signal {signal}"))
    .unwrap_or_default();
  let time = time
    .map(|time| format!(" --time {time}"))
    .unwrap_or_default();
  format!("docker stop{signal}{time} {container_name}")
}

fn convert_object_version(
  version: bollard::models::ObjectVersion,
) -> ObjectVersion {
  ObjectVersion {
    index: version.index,
  }
}

fn convert_driver(driver: bollard::models::Driver) -> Driver {
  Driver {
    name: driver.name,
    options: driver.options,
  }
}

fn convert_mount(mount: bollard::models::Mount) -> Mount {
  Mount {
    target: mount.target,
    source: mount.source,
    typ: mount.typ.map(convert_mount_type).unwrap_or_default(),
    read_only: mount.read_only,
    consistency: mount.consistency,
    bind_options: mount.bind_options.map(|options| {
      MountBindOptions {
        propagation: options
          .propagation
          .map(convert_mount_propogation)
          .unwrap_or_default(),
        non_recursive: options.non_recursive,
        create_mountpoint: options.create_mountpoint,
        read_only_non_recursive: options.read_only_non_recursive,
        read_only_force_recursive: options.read_only_force_recursive,
      }
    }),
    volume_options: mount.volume_options.map(|options| {
      MountVolumeOptions {
        no_copy: options.no_copy,
        labels: options.labels.unwrap_or_default(),
        driver_config: options.driver_config.map(|config| {
          MountVolumeOptionsDriverConfig {
            name: config.name,
            options: config.options.unwrap_or_default(),
          }
        }),
        subpath: options.subpath,
      }
    }),
    tmpfs_options: mount.tmpfs_options.map(|options| {
      MountTmpfsOptions {
        size_bytes: options.size_bytes,
        mode: options.mode,
      }
    }),
  }
}

fn convert_mount_type(typ: bollard::config::MountType) -> MountType {
  match typ {
    bollard::config::MountType::BIND => MountType::Bind,
    bollard::config::MountType::VOLUME => MountType::Volume,
    bollard::config::MountType::IMAGE => MountType::Image,
    bollard::config::MountType::TMPFS => MountType::Tmpfs,
    bollard::config::MountType::NPIPE => MountType::Npipe,
    bollard::config::MountType::CLUSTER => MountType::Cluster,
  }
}

fn convert_mount_propogation(
  propogation: bollard::config::MountBindOptionsPropagationEnum,
) -> MountBindOptionsPropagationEnum {
  match propogation {
    bollard::config::MountBindOptionsPropagationEnum::EMPTY => {
      MountBindOptionsPropagationEnum::Empty
    }
    bollard::config::MountBindOptionsPropagationEnum::PRIVATE => {
      MountBindOptionsPropagationEnum::Private
    }
    bollard::config::MountBindOptionsPropagationEnum::RPRIVATE => {
      MountBindOptionsPropagationEnum::Rprivate
    }
    bollard::config::MountBindOptionsPropagationEnum::SHARED => {
      MountBindOptionsPropagationEnum::Shared
    }
    bollard::config::MountBindOptionsPropagationEnum::RSHARED => {
      MountBindOptionsPropagationEnum::Rshared
    }
    bollard::config::MountBindOptionsPropagationEnum::SLAVE => {
      MountBindOptionsPropagationEnum::Slave
    }
    bollard::config::MountBindOptionsPropagationEnum::RSLAVE => {
      MountBindOptionsPropagationEnum::Rslave
    }
  }
}

fn convert_health_config(
  config: bollard::models::HealthConfig,
) -> HealthConfig {
  HealthConfig {
    test: config.test.unwrap_or_default(),
    interval: config.interval,
    timeout: config.timeout,
    retries: config.retries,
    start_period: config.start_period,
    start_interval: config.start_interval,
  }
}

fn convert_resources_ulimits(
  ulimit: bollard::models::ResourcesUlimits,
) -> ResourcesUlimits {
  ResourcesUlimits {
    name: ulimit.name,
    soft: ulimit.soft,
    hard: ulimit.hard,
  }
}

fn convert_resource_object(
  object: bollard::models::ResourceObject,
) -> ResourceObject {
  ResourceObject {
    nano_cpus: object.nano_cpus,
    memory_bytes: object.memory_bytes,
    generic_resources: object
      .generic_resources
      .map(convert_generic_resources),
  }
}

fn convert_generic_resources(
  resources: Vec<bollard::models::GenericResourcesInner>,
) -> Vec<GenericResourcesInner> {
  resources
    .into_iter()
    .map(|resource| GenericResourcesInner {
      named_resource_spec: resource.named_resource_spec.map(|spec| {
        GenericResourcesInnerNamedResourceSpec {
          kind: spec.kind,
          value: spec.value,
        }
      }),
      discrete_resource_spec: resource.discrete_resource_spec.map(
        |spec| GenericResourcesInnerDiscreteResourceSpec {
          kind: spec.kind,
          value: spec.value,
        },
      ),
    })
    .collect()
}

fn convert_platform(platform: bollard::models::Platform) -> Platform {
  Platform {
    architecture: platform.architecture,
    os: platform.os,
  }
}

fn convert_endpoint_spec_ports(
  ports: Vec<bollard::models::EndpointPortConfig>,
) -> Vec<EndpointPortConfig> {
  ports
    .into_iter()
    .map(|port| EndpointPortConfig {
      name: port.name,
      protocol: port.protocol.map(|protocol| match protocol {
        bollard::config::EndpointPortConfigProtocolEnum::EMPTY => EndpointPortConfigProtocolEnum::EMPTY,
        bollard::config::EndpointPortConfigProtocolEnum::TCP => EndpointPortConfigProtocolEnum::TCP,
        bollard::config::EndpointPortConfigProtocolEnum::UDP => EndpointPortConfigProtocolEnum::UDP,
        bollard::config::EndpointPortConfigProtocolEnum::SCTP => EndpointPortConfigProtocolEnum::SCTP,
      }),
      target_port: port.target_port,
      published_port: port.published_port,
      publish_mode: port.publish_mode.map(|protocol| match protocol {
        bollard::config::EndpointPortConfigPublishModeEnum::EMPTY => EndpointPortConfigPublishModeEnum::EMPTY,
        bollard::config::EndpointPortConfigPublishModeEnum::INGRESS => EndpointPortConfigPublishModeEnum::INGRESS,
        bollard::config::EndpointPortConfigPublishModeEnum::HOST => EndpointPortConfigPublishModeEnum::HOST,
      }),
    })
    .collect()
}

fn convert_task_spec(spec: bollard::models::TaskSpec) -> TaskSpec {
  TaskSpec {
    plugin_spec: spec.plugin_spec.map(|spec| TaskSpecPluginSpec {
      name: spec.name,
      remote: spec.remote,
      disabled: spec.disabled,
      plugin_privilege: spec.plugin_privilege.map(|privileges| {
        privileges
          .into_iter()
          .map(|privilege| PluginPrivilege {
            name: privilege.name,
            description: privilege.description,
            value: privilege.value,
          })
          .collect()
      }),
    }),
    container_spec: spec
      .container_spec
      .map(convert_task_spec_container_spec),
    network_attachment_spec: spec.network_attachment_spec.map(
      |spec| TaskSpecNetworkAttachmentSpec {
        container_id: spec.container_id,
      },
    ),
    resources: spec.resources.map(|resources| TaskSpecResources {
      limits: resources.limits.map(|limits| Limit {
        nano_cpus: limits.nano_cpus,
        memory_bytes: limits.memory_bytes,
        pids: limits.pids,
      }),
      reservations: resources
        .reservations
        .map(convert_resource_object),
    }),
    restart_policy: spec.restart_policy.map(|policy| {
      TaskSpecRestartPolicy {
        condition: policy
          .condition
          .map(convert_task_spec_restart_policy_condition),
        delay: policy.delay,
        max_attempts: policy.max_attempts,
        window: policy.window,
      }
    }),
    placement: spec.placement.map(|placement| TaskSpecPlacement {
      constraints: placement.constraints,
      preferences: placement.preferences.map(|preferences| {
        preferences
          .into_iter()
          .map(|preference| TaskSpecPlacementPreferences {
            spread: preference.spread.map(|spread| {
              TaskSpecPlacementSpread {
                spread_descriptor: spread.spread_descriptor,
              }
            }),
          })
          .collect()
      }),
      max_replicas: placement.max_replicas,
      platforms: placement.platforms.map(|platforms| {
        platforms.into_iter().map(convert_platform).collect()
      }),
    }),
    force_update: spec.force_update,
    runtime: spec.runtime,
    networks: spec.networks.map(|networks| {
      networks
        .into_iter()
        .map(|network| NetworkAttachmentConfig {
          target: network.target,
          aliases: network.aliases,
          driver_opts: network.driver_opts,
        })
        .collect()
    }),
    log_driver: spec.log_driver.map(|driver| TaskSpecLogDriver {
      name: driver.name,
      options: driver.options,
    }),
  }
}

fn convert_task_spec_container_spec(
  spec: bollard::models::TaskSpecContainerSpec,
) -> TaskSpecContainerSpec {
  TaskSpecContainerSpec {
    image: spec.image,
    labels: spec.labels,
    command: spec.command,
    args: spec.args,
    hostname: spec.hostname,
    env: spec.env,
    dir: spec.dir,
    user: spec.user,
    groups: spec.groups,
    privileges: spec.privileges.map(|privilege| {
      TaskSpecContainerSpecPrivileges {
        credential_spec: privilege.credential_spec.map(|spec| {
          TaskSpecContainerSpecPrivilegesCredentialSpec {
            config: spec.config,
            file: spec.file,
            registry: spec.registry,
          }
        }),
        se_linux_context: privilege.se_linux_context.map(|context| {
          TaskSpecContainerSpecPrivilegesSeLinuxContext {
            disable: context.disable,
            user: context.user,
            role: context.role,
            typ: context.typ,
            level: context.level,
          }
        }),
        seccomp: privilege.seccomp.map(|seccomp| {
          TaskSpecContainerSpecPrivilegesSeccomp {
            mode: seccomp.mode.map(|mode| match mode {
              bollard::config::TaskSpecContainerSpecPrivilegesSeccompModeEnum::EMPTY => TaskSpecContainerSpecPrivilegesSeccompModeEnum::EMPTY,
              bollard::config::TaskSpecContainerSpecPrivilegesSeccompModeEnum::DEFAULT => TaskSpecContainerSpecPrivilegesSeccompModeEnum::DEFAULT,
              bollard::config::TaskSpecContainerSpecPrivilegesSeccompModeEnum::UNCONFINED => TaskSpecContainerSpecPrivilegesSeccompModeEnum::UNCONFINED,
              bollard::config::TaskSpecContainerSpecPrivilegesSeccompModeEnum::CUSTOM => TaskSpecContainerSpecPrivilegesSeccompModeEnum::CUSTOM,
            }),
            profile: seccomp.profile,
          }
        }),
        app_armor: privilege.app_armor.map(|app_armor| {
          TaskSpecContainerSpecPrivilegesAppArmor {
            mode: app_armor.mode.map(|mode| match mode {
              bollard::config::TaskSpecContainerSpecPrivilegesAppArmorModeEnum::EMPTY => TaskSpecContainerSpecPrivilegesAppArmorModeEnum::EMPTY,
              bollard::config::TaskSpecContainerSpecPrivilegesAppArmorModeEnum::DEFAULT => TaskSpecContainerSpecPrivilegesAppArmorModeEnum::DEFAULT,
              bollard::config::TaskSpecContainerSpecPrivilegesAppArmorModeEnum::DISABLED => TaskSpecContainerSpecPrivilegesAppArmorModeEnum::DISABLED,
            }),
          }
        }),
        no_new_privileges: privilege.no_new_privileges,
      }
    }),
    tty: spec.tty,
    open_stdin: spec.open_stdin,
    read_only: spec.read_only,
    mounts: spec.mounts.map(|mounts| mounts.into_iter().map(convert_mount).collect()),
    stop_signal: spec.stop_signal,
    stop_grace_period: spec.stop_grace_period,
    health_check: spec.health_check.map(convert_health_config),
    hosts: spec.hosts,
    dns_config: spec.dns_config.map(|config| TaskSpecContainerSpecDnsConfig {
      nameservers: config.nameservers,
      search: config.search,
      options: config.options,
    }),
    secrets: spec.secrets.map(|secrets| secrets.into_iter().map(|secret| TaskSpecContainerSpecSecrets {
      file: secret.file.map(|file| TaskSpecContainerSpecFile {
        name: file.name,
        uid: file.uid,
        gid: file.gid,
        mode: file.mode,
      }),
      secret_id: secret.secret_id,
      secret_name: secret.secret_name,
    }).collect()),
    oom_score_adj: spec.oom_score_adj,
    configs: spec.configs.map(|configs| configs.into_iter().map(|config| TaskSpecContainerSpecConfigs {
      file: config.file.map(|file| TaskSpecContainerSpecFile {
        name: file.name,
        uid: file.uid,
        gid: file.gid,
        mode: file.mode,
      }),
      config_id: config.config_id,
      config_name: config.config_name,
    }).collect()),
    isolation: spec.isolation.map(|isolation| match isolation {
      bollard::config::TaskSpecContainerSpecIsolationEnum::DEFAULT => TaskSpecContainerSpecIsolationEnum::DEFAULT,
      bollard::config::TaskSpecContainerSpecIsolationEnum::PROCESS => TaskSpecContainerSpecIsolationEnum::PROCESS,
      bollard::config::TaskSpecContainerSpecIsolationEnum::HYPERV => TaskSpecContainerSpecIsolationEnum::HYPERV,
      bollard::config::TaskSpecContainerSpecIsolationEnum::EMPTY => TaskSpecContainerSpecIsolationEnum::EMPTY,
    }),
    init: spec.init,
    sysctls: spec.sysctls,
    capability_add: spec.capability_add,
    capability_drop: spec.capability_drop,
    ulimits: spec.ulimits.map(|ulimits| ulimits.into_iter().map(convert_resources_ulimits).collect()),
  }
}

fn convert_task_spec_restart_policy_condition(
  condition: bollard::config::TaskSpecRestartPolicyConditionEnum,
) -> TaskSpecRestartPolicyConditionEnum {
  match condition {
    bollard::config::TaskSpecRestartPolicyConditionEnum::EMPTY => TaskSpecRestartPolicyConditionEnum::EMPTY,
    bollard::config::TaskSpecRestartPolicyConditionEnum::NONE => TaskSpecRestartPolicyConditionEnum::NONE,
    bollard::config::TaskSpecRestartPolicyConditionEnum::ON_FAILURE => TaskSpecRestartPolicyConditionEnum::ON_FAILURE,
    bollard::config::TaskSpecRestartPolicyConditionEnum::ANY => TaskSpecRestartPolicyConditionEnum::ANY,
  }
}

fn convert_tls_info(tls_info: bollard::models::TlsInfo) -> TlsInfo {
  TlsInfo {
    trust_root: tls_info.trust_root,
    cert_issuer_subject: tls_info.cert_issuer_subject,
    cert_issuer_public_key: tls_info.cert_issuer_public_key,
  }
}

#[cfg(test)]
mod tests {
  use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
  };

  use super::{
    docker_login, docker_login_args,
    redact_docker_login_token_fragments,
  };

  fn path_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
  }

  struct PathGuard {
    original_path: Option<OsString>,
    temp_dir: PathBuf,
  }

  impl Drop for PathGuard {
    fn drop(&mut self) {
      unsafe {
        match &self.original_path {
          Some(path) => env::set_var("PATH", path),
          None => env::remove_var("PATH"),
        }
      }
      let _ = fs::remove_dir_all(&self.temp_dir);
    }
  }

  fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    env::temp_dir()
      .join(format!("komodo-{name}-{}-{nanos}", std::process::id()))
  }

  fn install_fake_docker(script: &str) -> PathGuard {
    let temp_dir = unique_temp_dir("docker-login-test");
    fs::create_dir_all(&temp_dir).unwrap();

    let docker_path = temp_dir.join("docker");
    fs::write(&docker_path, script).unwrap();

    let mut permissions =
      fs::metadata(&docker_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&docker_path, permissions).unwrap();

    let original_path = env::var_os("PATH");
    let mut paths = vec![temp_dir.clone()];
    if let Some(path) = &original_path {
      paths.extend(env::split_paths(path));
    }
    let new_path = env::join_paths(paths).unwrap();

    unsafe {
      env::set_var("PATH", &new_path);
    }

    PathGuard {
      original_path,
      temp_dir,
    }
  }

  fn install_path_without_docker() -> PathGuard {
    let temp_dir = unique_temp_dir("docker-login-missing");
    fs::create_dir_all(&temp_dir).unwrap();
    let original_path = env::var_os("PATH");

    unsafe {
      env::set_var("PATH", &temp_dir);
    }

    PathGuard {
      original_path,
      temp_dir,
    }
  }

  #[test]
  fn docker_login_args_do_not_include_token() {
    let token = "abc$def' ghi`jkl";
    let args = docker_login_args("registry.example.com", "user");

    assert!(!args.join(" ").contains(token));
    assert!(args.contains(&"--password-stdin".to_string()));
  }

  #[test]
  fn docker_login_output_redacts_delimited_short_token_fragments() {
    let redacted = redact_docker_login_token_fragments(
      "single:a\npair:bc\ntriple:123\nquoted:\"XY\"\nword:stacktrace\n",
      "abc123XYZ",
    );

    assert_eq!(
      redacted,
      "single:[REDACTED]\npair:[REDACTED]\ntriple:[REDACTED]\nquoted:\"[REDACTED]\"\nword:stacktrace\n"
    );
  }

  #[tokio::test]
  async fn docker_login_writes_shell_special_token_to_stdin_unchanged()
   {
    let _lock = path_lock()
      .lock()
      .unwrap_or_else(|poison| poison.into_inner());
    let temp_dir = unique_temp_dir("docker-login-stdin");
    fs::create_dir_all(&temp_dir).unwrap();

    let stdin_path = temp_dir.join("stdin.txt");
    let args_path = temp_dir.join("args.txt");
    let _path_guard = install_fake_docker(&format!(
      "#!/bin/sh\ncat > \"{}\"\nprintf '%s\\n' \"$@\" > \"{}\"\n",
      stdin_path.display(),
      args_path.display()
    ));

    let token = "pa$$ word 'quoted' \"double\" `backtick`\nline two";
    let logged_in =
      docker_login("registry.example.com", "user", Some(token))
        .await
        .unwrap();

    assert!(logged_in);
    assert_eq!(fs::read_to_string(stdin_path).unwrap(), token);
    assert_eq!(
      fs::read_to_string(args_path).unwrap(),
      "login\nregistry.example.com\n--username\nuser\n--password-stdin\n"
    );

    let _ = fs::remove_dir_all(temp_dir);
  }

  #[tokio::test]
  async fn docker_login_error_redacts_token_fragments_from_output() {
    let _lock = path_lock()
      .lock()
      .unwrap_or_else(|poison| poison.into_inner());
    let _path_guard = install_fake_docker(
      "#!/bin/sh\ntoken=\"$(cat)\"\nprintf 'prefix:%s\\n' \"$(printf '%s' \"$token\" | cut -c1-15)\" >&2\nprintf 'suffix:%s\\n' \"$(printf '%s' \"$token\" | tail -c 19)\" >&2\nprintf 'line:%s\\n' \"$(printf '%s' \"$token\" | tail -n 1)\" >&2\nexit 1\n",
    );

    let token = "pa$$ word 'quoted' \"double\" `backtick`\nline two";
    let error =
      docker_login("registry.example.com", "user", Some(token))
        .await
        .unwrap_err();
    let displayed = format!("{error:#}");

    assert!(
      displayed.contains("Registry registry.example.com login error")
    );
    assert!(!displayed.contains("pa$$ word 'quot"));
    assert!(!displayed.contains("\" `backtick`"));
    assert!(!displayed.contains("line two"));
  }

  #[tokio::test]
  async fn docker_login_spawn_failures_are_human_readable() {
    let _lock = path_lock()
      .lock()
      .unwrap_or_else(|poison| poison.into_inner());
    let _path_guard = install_path_without_docker();

    let error = docker_login(
      "registry.example.com",
      "user",
      Some("pa$$ word 'quoted'"),
    )
    .await
    .unwrap_err();
    let displayed = format!("{error:#}");

    assert!(
      displayed.contains("Registry registry.example.com login error")
    );
    assert!(
      displayed.contains("Failed to start docker login command")
    );
    assert!(!displayed.contains("Os {"));
    assert!(!displayed.contains("pa$$ word 'quoted'"));
  }
}

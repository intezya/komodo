# Docker Compose

Komodo can deploy Docker Compose projects through the `Stack` resource.

## Configuration

```toml
[[stack]]
name = "my-stack"
[stack.config]
server = "server-prod"
run_directory = "/opt/stacks/my-stack"
file_paths = ["compose.yaml"]
git_account = "my-user"
repo = "myorg/stacks"
environment = """
DB_HOST = db.example.com
LOG_LEVEL = info
"""
```

### Config fields

| Field | Description | Default |
|---|---|---|
| `server` | The Server to deploy on. | — |
| `file_paths` | List of compose files. Supports composing multiple files via `docker compose -f ... -f ...`. | `[]` |
| `run_directory` | Working directory for compose commands. | — |
| `project_name` | Override the compose project name. Defaults to the Stack name. | Stack name |
| `environment` | Environment variables written to a `.env` file and passed via `--env-file`. Supports [variable interpolation](../configuration/variables.md). | `""` |
| `extra_args` | Additional flags passed to `docker compose up`. | `""` |
| `rolling_update` | Replace service replicas one at a time while keeping existing replicas available. | `false` |
| `ignore_services` | Services to exclude from health checks (e.g. init containers that exit after startup). | `[]` |
| `git_provider` | Git provider domain. | `github.com` |
| `git_account` | Git provider account for private repos. | — |
| `repo` | Repository in `owner/repo` format. | — |
| `branch` | Branch to clone. | `main` |
| `auto_update` | Automatically redeploy when newer image digests are available. | `false` |
| `poll_for_updates` | Check for newer images and show an update indicator. | `false` |
| `auto_deploy_git_updates` | Reconcile tracked Git file changes while the Stack is running. Independent of image polling. | `false` |
| `send_alerts` | Send alerts on stack state changes. | `true` |
| `links` | Quick links displayed in the resource header. | `[]` |

## Defining Compose Files

Stacks support three ways to provide compose files:

1. **Write in the UI** — Komodo writes the files to the host at deploy time.
2. **Files on the host** — Point to existing files on the server.
3. **Git repo** — Komodo clones the repo onto the host to deploy. Changes are tracked in git and you can use [webhooks](../automate/webhooks.md) to auto-redeploy on push.

Git-backed Stacks can instead use the opt-in [GitOps controller](../automate/gitops-controller.md). It polls moving branches, only reconciles an already running Stack, and removes orphans only during a full automatic Compose reconciliation.

## Replicas and Rolling Updates

Komodo reads `deploy.replicas` from the resolved Compose configuration and
monitors every container belonging to the service:

```yaml
services:
  web:
    image: ghcr.io/example/web:latest
    deploy:
      replicas: 3
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
      interval: 5s
      timeout: 2s
      retries: 6
```

Enable **Rolling Update** on the Stack to replace replicas sequentially. Komodo
starts one additional replica, waits up to 60 seconds for its healthcheck,
then stops one old replica. Without a healthcheck, the new container must stay
running for 10 seconds.

Rolling services cannot use `container_name` or published host ports because
the old and new containers must run at the same time. Use an internal reverse
proxy such as Traefik and opt incompatible singleton services out explicitly:

```yaml
services:
  database:
    container_name: database
    labels:
      komodo.rollout: "false"
```

For connection draining, add a hook that runs in each old container before it
is stopped:

```yaml
labels:
  komodo.rollout.pre-stop-hook: "touch /tmp/drain && sleep 10"
```

`rolling_update` cannot be combined with `destroy_before_deploy`.

## Importing Existing Projects

To import a running compose project, create a Stack in Komodo with access to the same compose files and attach the correct Server. Komodo matches projects by compose project name — if the running project name differs from the Stack name, set a custom `project_name` in the config. Run `docker compose ls` on the host to find existing project names.

## Deploying to a Swarm

A Stack can target a **Swarm** instead of a single Server to deploy via `docker stack deploy`. See [Swarm](../swarm.md) for details.

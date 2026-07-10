# Pull-based GitOps controller

The GitOps controller polls opted-in, Git-backed Resource Syncs and Stacks on
`resource_poll_interval`. Both controls default to `false`; existing manual,
webhook, and image-digest update behavior is unchanged until an operator
enables a control.

## Enable a Resource Sync

```toml
[[resource_sync]]
name = "de1"

[resource_sync.config]
git_account = "automation"
repo = "example/infrastructure"
branch = "main"
resource_path = ["resources/de1.toml"]
auto_apply_updates = true
```

The controller reads each effective `(provider, account, repo, branch)` once
per cycle, including sources shared by several Stacks or Resource Syncs. A
linked Repo resolves to that Repo's source. Fixed commits, files on the host,
and UI-defined files are not eligible for polling.

| Diff | Controller action |
|---|---|
| Create | Applies it. |
| Update | Applies it. |
| Delete | Leaves it pending for manual review. |

This applies to resources, variables, and user groups. A mixed change is not
rejected because it contains a Delete: the safe Create and Update subset runs,
while the Delete remains visible in the pending diff. Git authors therefore
receive configuration write authority; restrict repository write access
accordingly.

For a newly created Stack, its TOML `deploy` value still controls whether the
sync starts it. `deploy = false` creates the Stack without starting it.

## Enable a Stack

```toml
[[stack]]
name = "pocket-id"

[stack.config]
server = "de1"
repo = "example/infrastructure"
branch = "main"
run_directory = "stacks/pocket-id"
file_paths = ["compose.yaml"]
auto_deploy_git_updates = true
```

Only an already **Running** Stack is reconciled. Stopped, paused, unknown, and
newly created Stacks are left unchanged. The controller compares the tracked
file contents, so unrelated commits do not deploy a Stack. A removed tracked
file requires a full reconciliation; an automatic full Compose update removes
orphans, while a service-scoped update does not.

Image digest polling (`poll_for_updates` and `auto_update`) is independent of
Git polling. `auto_pull` only applies when a deployment is already running.

## Failures, retries, and rollback

A failed Git source, invalid TOML, missing file, or invalid Compose input does
not change runtime state. The affected source group is retried on the next
cycle; other source groups continue. Tokens are obtained through the configured
Git provider account and are never written to controller logs.

The controller does not automatically roll back a deployment that passed
Compose validation but failed during `docker compose up`. Revert the Git
commit or use the existing manual Stack and Resource Sync actions. Migrate
deploy-on-push workflows gradually: enable one Resource Sync first, observe
pending changes and Updates, then enable Stack reconciliation one running Stack
at a time.

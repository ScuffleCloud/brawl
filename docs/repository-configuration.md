# Repository Configuration

This file refers to the `.github/brawl.toml` file used to configure brawl for a given repository. If you are looking for a server configuration reference, please refer to the [**`server configuration reference`**](./configuration.md).

The configuration here is for the [**`GitHubBrawlRepoConfig`**](../server/src/github/config.rs) struct.

## Configuration Reference

### Enabled

The `enabled` option is a boolean that determines if brawl is enabled for this repository.

This defaults to `true`.

### Labels

The `labels` section is used to specify the labels to attach to PRs on different states.

#### On Merge Queued

The `on_merge_queued` option is a list of labels to attach to PRs when they are in the merge queue.

This defaults to an empty list.

These labels will be added to a PR when a it has been added to the merge queue before the merge has started.

#### On Merge In Progress

The `on_merge_in_progress` option is a list of labels to attach to PRs when they are being merged.

This defaults to an empty list.

These labels will be added to a PR when a it has started merging.

#### On Merge Failure

The `on_merge_failure` option is a list of labels to attach to PRs when they fail to merge.

This defaults to an empty list.

These labels will be added to a PR when a it has failed to merge.

#### On Merge Success

The `on_merge_success` option is a list of labels to attach to PRs when they are merged.

This defaults to an empty list.

These labels will be added to a PR when a it has been merged.

#### On Try In Progress

The `on_try_in_progress` option is a list of labels to attach to PRs when they are being tried.

This defaults to an empty list.

These labels will be added to a PR when a it has started trying.

#### On Try Failure

The `on_try_failure` option is a list of labels to attach to PRs when they fail to try.

This defaults to an empty list.

These labels will be added to a PR when a it has failed to try.

#### Auto-try Enabled

The `auto_try_enabled` option is a list of labels to attach to PRs when auto-try is enabled.

This defaults to an empty list.

These labels will be added to a PR when auto-try is enabled.

### Branches

The `branches` option is a list of branches that brawl will listen to.

This defaults to an empty list.

These branches will be used to determine if a PR should be merged or tried.

### Try Branch Prefix

The `try_branch_prefix` option is the prefix for the branch name used when trying a PR.

This defaults to `automation/brawl/try/`.

When creating a temporary branch for trying a PR, brawl will use this prefix in combination with the PR number to create the branch name.

For example, if the PR number is `123` and the prefix is `automation/brawl/try/`, the branch name will be `automation/brawl/try/123`.

Brawl will strip any trailing `/` from the prefix and always combine it with the PR number. So `automation/brawl/try/` and `automation/brawl/try` are the same.

### Merge Branch Prefix

The `merge_branch_prefix` option is the prefix for the branch name used when merging a PR.

This defaults to `automation/brawl/merge/`.

When creating a temporary branch for merging a PR, brawl will use this prefix in combination with the target branch name to create the branch name.

For example, if the target branch is `main` and the prefix is `automation/brawl/merge/`, the branch name will be `automation/brawl/merge/main`.

Brawl will strip any trailing `/` from the prefix and always combine it with the target branch name. So `automation/brawl/merge/` and `automation/brawl/merge` are the same.

### Temp Branch Prefix

The `temp_branch_prefix` option is the prefix for the branch name used when creating a temporary branch for help in creating a merge commit. Due to the limitations of the GitHub API we need to create a temporary branch to create a merge commit. Technically we could get around this by using the git api directly but we avoid using `git` for now to keep the process simple.

This defaults to `automation/brawl/temp/`.

So when we are doing either a merge or a try, we will create a temporary branch in the form of `automation/brawl/temp/<uuid>` and then perform a push to push this branch to the base and then merge the head into the base to create a merge commit and then delete the branch.

This branch is never persisted and is deleted immediately after we get a commit sha.

### Merge Permissions

The `merge_permissions` option is a list of permissions required to merge a PR.

This defaults to `["role:write"]`.

A user must have at least one of these permissions in order to use the `brawl merge` command.

### Try Permissions

The `try_permissions` option is a list of permissions required to try a PR.

This defaults to the same as the [**`merge_permissions`**](./repository-configuration.md#merge-permissions) option.

A user must have at least one of these permissions in order to use the `brawl try` command.

### Reviewer Permissions

The `reviewer_permissions` option is a list of permissions required to be a reviewer.

This defaults to the same as the [**`merge_permissions`**](./repository-configuration.md#merge-permissions) option.

A user must have at least one of these permissions in order to be saved as a reviewer when doing a merge.

The reason for this is that on GitHub anyone can review a PR so we can't just use every single person who reviews the PR because they might not be a member of the repository, this is a way to specify which permissions users need in order for us to see them as a valid reviewer.

### Required Status Checks

The `required_status_checks` option is a list of status checks that must be successful before a PR can be merged.

This defaults to `["brawl-done"]`.

If this list is empty, the PR will be merged immediately.
The PR will not be merged until all of the status checks are successful, if any of the status checks fail the PR will immediately fail and report a failure. If all of the status checks are not successful within the [**`timeout_minutes`**](./repository-configuration.md#timeout-minutes) the PR will fail to merge.

### Timeout Minutes

The `timeout_minutes` option is the number of minutes to wait before declaring the merge failed if the required status checks are not met.

This defaults to `60`.

If the required status checks are not met within this time, the PR will fail to merge.

### Max Reviewers

The `max_reviewers` option is the maximum number of reviewers to request for a PR.

This defaults to `10`.

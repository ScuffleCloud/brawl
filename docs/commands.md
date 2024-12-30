# Commands

This file refers to the commands available to you when using brawl for a given repository, assuming you already have a fully setup brawl server & GitHub app.

All brawl commands can be prefixed with either `?brawl`, `@brawl`, `/brawl`, or `>brawl`.

If you do not have permission to use a command brawl will just ignore it.

Similarly if there is an invalid command during the parsing brawl will ignore it and not run anything.

## Merge

The merge command is used to merge a PR into the target branch using the merge queue.

The syntax is as follows:

```
?brawl merge [p=<priority>]
```

The `p` argument is optional and defaults to 5. Priority changes the order of the PR in the merge queue.

When merging brawl will create a new branch using the  [**`merge branch prefix`**](./configuration.md#merge-branch-prefix) and the target branch name. It will create a new merge commit and push it to the target branch. Brawl will wait for all [**`required status checks`**](./configuration.md#required-status-checks) to pass before pushing the merge commit to the target branch. 

In order to use this command you must have one of the permissions in the [**`merge permissions`**](./configuration.md#merge-permissions) section, by default this is `role:write`, which means you must have write access to the repository.

## Cancel

The cancel command is used to cancel running operations on the current PR.

The syntax is as follows:

```
?brawl cancel
```

If the PR is being merged it will cancel the merge. If the PR is being tried it will cancel the try.

Depending on what is being cancelled, you will need to have the appropriate permissions.

For merging that would be the [**`merge permissions`**](./configuration.md#merge-permissions) section. 

For trying that would be the [**`try permissions`**](./configuration.md#try-permissions) section.

## Try

The try command is used to try a PR, which is similar to merge except it does not do the final push to the target branch if the run is successful.

The syntax is as follows:

```
?brawl try [(commit|c|head|h)=<commit-sha>] [(base|b)=<base-sha>]
```

The `commit` and `base` arguments are optional and default to the current commit (PR HEAD) and the target branch (PR BASE).

Commit has aliases `commit`, `c`, `head`, and `h`.

Base has aliases `base`, `b`.

The behavior around the commit and base is as follows:

- If you specify neither a head or base brawl will use the current PR's HEAD (latest commit on the PR) and BASE (target branch commit).
- If you specify a head but no base, brawl will use the head commit as the base commit (testing without checking merge conflicts).
- If you specify a base but no head, brawl will use the latest commit on the PR as the head commit (testing merging the PR into the provided base commit).
- If you specify both a head and base, brawl will use the provided head commit as the head commit and the provided base commit as the base commit.

Trying is useful if you want to see if a PR merge will pass all the required status checks without actually merging it.

In order to use this command you must have one of the permissions in the [**`try permissions`**](./configuration.md#try-permissions) section, by default this is the same as whatever the [**`merge permissions`**](./configuration.md#merge-permissions) section is set to.

Like a merge, the bot will wait for all [**`required status checks`**](./configuration.md#required-status-checks) to pass before reporting the result.

## Retry

The retry command is used to retry a failed run.

The syntax is as follows:

```
?brawl retry
```

This will retry the previous run.

If the previous run was a merge it will retry the merge, and you will need one of the permissions in the [**`merge permissions`**](./configuration.md#merge-permissions) section.

If the previous run was a try it will retry the try, and you will need one of the permissions in the [**`try permissions`**](./configuration.md#try-permissions) section.

## Ping

The ping command is used to ping the brawl server.

The syntax is as follows:

```
?brawl ping
```

This will send a ping to the brawl server and return a pong.

## Auto-try

The auto-try command is used to enable or disable auto-try.

To enable auto-try you can use the following command:

```
?brawl auto-try
```

And to disable auto-try you can use the following command:

```
?brawl !auto-try
```

Auto-try is useful if you want to try the PR on every push to the base branch. (similar to how GitHub Actions `push` works)

You will need to have one of the permissions in the [**`try permissions`**](./configuration.md#try-permissions) section.

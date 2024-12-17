
CREATE TYPE github_pr_status AS ENUM (
    'open',
    'closed',
    'draft'
);

CREATE TYPE github_pr_merge_status AS ENUM (
    -- Not ready to merge
    'not_ready',
    -- Can be merged
    'ready',
    -- Has a merge conflict
    'conflict',
    -- CI Checks failed prior to merging
    'check_failure',
    -- Merge failed during the run
    'merge_failure',
    -- Merged
    'merged'
);

CREATE TABLE github_pr (
    github_repo_id BIGINT NOT NULL,
    github_pr_number INT NOT NULL,

    -- The title of the PR (on GitHub)
    title TEXT NOT NULL,
    -- The body of the PR (on GitHub)
    body TEXT NOT NULL,

    -- The merge status of the PR (on GitHub)
    merge_status github_pr_merge_status NOT NULL,
    -- The ID of the user who created the PR (on GitHub)
    author_id BIGINT NOT NULL,
    -- The IDs of the users who reviewed the PR (via the Brawl command) (max 10 - no nulls)
    reviewer_ids BIGINT[] NOT NULL CHECK (array_length(reviewer_ids, 1) <= 10 AND array_position(reviewer_ids, NULL) IS NULL),
    -- The IDs of the users who are assigned to the PR (on GitHub) (max 10 - no nulls)
    assigned_ids BIGINT[] NOT NULL CHECK (array_length(assigned_ids, 1) <= 10 AND array_position(assigned_ids, NULL) IS NULL),
    -- The status of the PR (on GitHub)
    status github_pr_status NOT NULL,
    -- The SHA of the merge commit (if the PR was merged)
    merge_commit_sha TEXT,
    -- The target branch of the PR
    target_branch TEXT NOT NULL,
    -- The source branch of the PR
    source_branch TEXT NOT NULL,
    -- The SHA of the latest commit on the PR
    latest_commit_sha TEXT NOT NULL,
    -- The time the PR was created
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The time the PR was last updated
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- The default priority of the PR
    default_priority INT,

    PRIMARY KEY (github_repo_id, github_pr_number)
);


CREATE TYPE github_ci_run_status AS ENUM (
    -- We havent started the run yet.
    'queued',
    -- We have started the run but github has not yet started the CI run.
    'pending',
    -- We have started the run and github has started the CI run.
    'running',
    -- The CI run completed successfully.
    'success',
    -- The CI run failed.
    'failure',
    -- The CI run was cancelled.
    'cancelled'
);

CREATE TABLE github_ci_runs (
    id SERIAL PRIMARY KEY,
    github_repo_id BIGINT NOT NULL,
    github_pr_number INT NOT NULL,
    status github_ci_run_status NOT NULL DEFAULT 'queued',
    -- The base of the run:
    -- Must start with either `branch:` or `commit:` followed by the branch name or commit SHA
    base_ref TEXT NOT NULL,
    -- The SHA of the head commit (the commit we are merging from)
    head_commit_sha TEXT NOT NULL,
    -- The SHA of the commit that was run (if the run was successful), null if the run has not started yet.
    run_commit_sha TEXT,
    -- A concurrency group only allows one CI run to be active at a time.
    ci_branch TEXT NOT NULL,
    -- The priority of the CI run (higher priority runs are run first)
    priority INT NOT NULL,
    -- The ID of the user who requested the CI run (on GitHub)
    requested_by_id BIGINT NOT NULL,
    -- Is dry run?
    is_dry_run BOOLEAN NOT NULL,
    -- The time the CI run was completed
    completed_at TIMESTAMPTZ,
    -- The time the CI run was started
    started_at TIMESTAMPTZ,
    -- The time the CI run was created
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The time the CI run was last updated
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The ID of the user who approved the CI run
    approved_by_ids BIGINT[] NOT NULL CHECK (array_length(approved_by_ids, 1) <= 10 AND array_position(approved_by_ids, NULL) IS NULL),

    FOREIGN KEY (github_repo_id, github_pr_number) REFERENCES github_pr(github_repo_id, github_pr_number) ON DELETE CASCADE
);

-- This index enforces the concurrency group constraint, ensuring that only one CI run
-- can be running at a time on a given repository concurrency group.
CREATE UNIQUE INDEX github_ci_runs_ci_branch_idx ON github_ci_runs (github_repo_id, ci_branch) WHERE status = 'pending' OR status = 'running';

-- This index ensures that only one CI run can be running at a time for a given PR.
CREATE UNIQUE INDEX github_ci_runs_pr_idx ON github_ci_runs (github_repo_id, github_pr_number) WHERE completed_at IS NULL;

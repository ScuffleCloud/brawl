#![cfg_attr(coverage_nightly, coverage(off))]
// @generated automatically by Diesel CLI.

/// A module containing custom SQL type definitions
///
/// (Automatically generated by Diesel.)
#[doc(hidden)]
pub mod sql_types {
    /// The `github_ci_run_status` SQL type
    ///
    /// (Automatically generated by Diesel.)
    #[derive(diesel::query_builder::QueryId, Clone, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "github_ci_run_status"))]
    #[doc(hidden)]
    pub struct GithubCiRunStatus;

    /// The `github_ci_run_status_check_status` SQL type
    ///
    /// (Automatically generated by Diesel.)
    #[derive(diesel::query_builder::QueryId, Clone, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "github_ci_run_status_check_status"))]
    #[doc(hidden)]
    pub struct GithubCiRunStatusCheckStatus;

    /// The `github_pr_merge_status` SQL type
    ///
    /// (Automatically generated by Diesel.)
    #[derive(diesel::query_builder::QueryId, Clone, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "github_pr_merge_status"))]
    #[doc(hidden)]
    pub struct GithubPrMergeStatus;

    /// The `github_pr_status` SQL type
    ///
    /// (Automatically generated by Diesel.)
    #[derive(diesel::query_builder::QueryId, Clone, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "github_pr_status"))]
    #[doc(hidden)]
    pub struct GithubPrStatus;
}

diesel::table! {
    use diesel::sql_types::*;
    use super::sql_types::GithubCiRunStatusCheckStatus;

    /// Representation of the `github_ci_run_status_checks` table.
    ///
    /// (Automatically generated by Diesel.)
    github_ci_run_status_checks (id) {
        /// The `id` column of the `github_ci_run_status_checks` table.
        ///
        /// Its SQL type is `Int8`.
        ///
        /// (Automatically generated by Diesel.)
        id -> Int8,
        /// The `github_ci_run_id` column of the `github_ci_run_status_checks` table.
        ///
        /// Its SQL type is `Int4`.
        ///
        /// (Automatically generated by Diesel.)
        github_ci_run_id -> Int4,
        /// The `status_check_name` column of the `github_ci_run_status_checks` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        status_check_name -> Text,
        /// The `status_check_status` column of the `github_ci_run_status_checks` table.
        ///
        /// Its SQL type is `GithubCiRunStatusCheckStatus`.
        ///
        /// (Automatically generated by Diesel.)
        status_check_status -> GithubCiRunStatusCheckStatus,
        /// The `started_at` column of the `github_ci_run_status_checks` table.
        ///
        /// Its SQL type is `Timestamptz`.
        ///
        /// (Automatically generated by Diesel.)
        started_at -> Timestamptz,
        /// The `completed_at` column of the `github_ci_run_status_checks` table.
        ///
        /// Its SQL type is `Nullable<Timestamptz>`.
        ///
        /// (Automatically generated by Diesel.)
        completed_at -> Nullable<Timestamptz>,
        /// The `url` column of the `github_ci_run_status_checks` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        url -> Text,
        /// The `required` column of the `github_ci_run_status_checks` table.
        ///
        /// Its SQL type is `Bool`.
        ///
        /// (Automatically generated by Diesel.)
        required -> Bool,
    }
}

diesel::table! {
    use diesel::sql_types::*;
    use super::sql_types::GithubCiRunStatus;

    /// Representation of the `github_ci_runs` table.
    ///
    /// (Automatically generated by Diesel.)
    github_ci_runs (id) {
        /// The `id` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Int4`.
        ///
        /// (Automatically generated by Diesel.)
        id -> Int4,
        /// The `github_repo_id` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Int8`.
        ///
        /// (Automatically generated by Diesel.)
        github_repo_id -> Int8,
        /// The `github_pr_number` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Int4`.
        ///
        /// (Automatically generated by Diesel.)
        github_pr_number -> Int4,
        /// The `status` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `GithubCiRunStatus`.
        ///
        /// (Automatically generated by Diesel.)
        status -> GithubCiRunStatus,
        /// The `base_ref` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        base_ref -> Text,
        /// The `head_commit_sha` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        head_commit_sha -> Text,
        /// The `run_commit_sha` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Nullable<Text>`.
        ///
        /// (Automatically generated by Diesel.)
        run_commit_sha -> Nullable<Text>,
        /// The `ci_branch` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        ci_branch -> Text,
        /// The `priority` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Int4`.
        ///
        /// (Automatically generated by Diesel.)
        priority -> Int4,
        /// The `approved_by_ids` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Array<Int8>`.
        ///
        /// (Manually changed from `Array<Nullable<Int8>>` to `Array<Int8>`)
        approved_by_ids -> Array<Int8>,
        /// The `requested_by_id` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Int8`.
        ///
        /// (Automatically generated by Diesel.)
        requested_by_id -> Int8,
        /// The `is_dry_run` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Bool`.
        ///
        /// (Automatically generated by Diesel.)
        is_dry_run -> Bool,
        /// The `completed_at` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Nullable<Timestamptz>`.
        ///
        /// (Automatically generated by Diesel.)
        completed_at -> Nullable<Timestamptz>,
        /// The `started_at` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Nullable<Timestamptz>`.
        ///
        /// (Automatically generated by Diesel.)
        started_at -> Nullable<Timestamptz>,
        /// The `created_at` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Timestamptz`.
        ///
        /// (Automatically generated by Diesel.)
        created_at -> Timestamptz,
        /// The `updated_at` column of the `github_ci_runs` table.
        ///
        /// Its SQL type is `Timestamptz`.
        ///
        /// (Automatically generated by Diesel.)
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    use diesel::sql_types::*;
    use super::sql_types::GithubPrMergeStatus;
    use super::sql_types::GithubPrStatus;

    /// Representation of the `github_pr` table.
    ///
    /// (Automatically generated by Diesel.)
    github_pr (github_repo_id, github_pr_number) {
        /// The `github_repo_id` column of the `github_pr` table.
        ///
        /// Its SQL type is `Int8`.
        ///
        /// (Automatically generated by Diesel.)
        github_repo_id -> Int8,
        /// The `github_pr_number` column of the `github_pr` table.
        ///
        /// Its SQL type is `Int4`.
        ///
        /// (Automatically generated by Diesel.)
        github_pr_number -> Int4,
        /// The `title` column of the `github_pr` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        title -> Text,
        /// The `body` column of the `github_pr` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        body -> Text,
        /// The `merge_status` column of the `github_pr` table.
        ///
        /// Its SQL type is `GithubPrMergeStatus`.
        ///
        /// (Automatically generated by Diesel.)
        merge_status -> GithubPrMergeStatus,
        /// The `author_id` column of the `github_pr` table.
        ///
        /// Its SQL type is `Int8`.
        ///
        /// (Automatically generated by Diesel.)
        author_id -> Int8,
        /// The `assigned_ids` column of the `github_pr` table.
        ///
        /// Its SQL type is `Array<Int8>`.
        ///
        /// (Manually changed from `Array<Nullable<Int8>>` to `Array<Int8>`)
        assigned_ids -> Array<Int8>,
        /// The `status` column of the `github_pr` table.
        ///
        /// Its SQL type is `GithubPrStatus`.
        ///
        /// (Automatically generated by Diesel.)
        status -> GithubPrStatus,
        /// The `merge_commit_sha` column of the `github_pr` table.
        ///
        /// Its SQL type is `Nullable<Text>`.
        ///
        /// (Automatically generated by Diesel.)
        merge_commit_sha -> Nullable<Text>,
        /// The `target_branch` column of the `github_pr` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        target_branch -> Text,
        /// The `source_branch` column of the `github_pr` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        source_branch -> Text,
        /// The `latest_commit_sha` column of the `github_pr` table.
        ///
        /// Its SQL type is `Text`.
        ///
        /// (Automatically generated by Diesel.)
        latest_commit_sha -> Text,
        /// The `created_at` column of the `github_pr` table.
        ///
        /// Its SQL type is `Timestamptz`.
        ///
        /// (Automatically generated by Diesel.)
        created_at -> Timestamptz,
        /// The `updated_at` column of the `github_pr` table.
        ///
        /// Its SQL type is `Timestamptz`.
        ///
        /// (Automatically generated by Diesel.)
        updated_at -> Timestamptz,
        /// The `default_priority` column of the `github_pr` table.
        ///
        /// Its SQL type is `Nullable<Int4>`.
        ///
        /// (Automatically generated by Diesel.)
        default_priority -> Nullable<Int4>,
        /// The `added_labels` column of the `github_pr` table.
        ///
        /// Its SQL type is `Array<Text>`.
        ///
        /// (Manually changed from `Array<Nullable<Text>>` to `Array<Text>`)
        added_labels -> Array<Text>,
    }
}

diesel::table! {
    /// Representation of the `health_check` table.
    ///
    /// (Automatically generated by Diesel.)
    health_check (id) {
        /// The `id` column of the `health_check` table.
        ///
        /// Its SQL type is `Int4`.
        ///
        /// (Automatically generated by Diesel.)
        id -> Int4,
        /// The `updated_at` column of the `health_check` table.
        ///
        /// Its SQL type is `Timestamptz`.
        ///
        /// (Automatically generated by Diesel.)
        updated_at -> Timestamptz,
    }
}

diesel::joinable!(github_ci_run_status_checks -> github_ci_runs (github_ci_run_id));

#[rustfmt::skip]
diesel::allow_tables_to_appear_in_same_query!(
    github_ci_run_status_checks,
    github_ci_runs,
    github_pr,
    health_check,
);

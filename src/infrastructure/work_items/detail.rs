//! Inbox detail and explicit user actions. Every request names its remote target.

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde_json::{Value, json};

use super::{array, github, linear, string, timestamp};
use crate::domain::work_items::{
    CheckRollup, DraftThread, FileViewed, MergeState, PrAction, ReviewVerdict, WorkCheck,
    WorkComment, WorkCommit, WorkCommits, WorkDetail, WorkDiff, WorkDiffFile, WorkItem, WorkKind,
    WorkSource, WorkThread,
};

const DETAIL_QUERY: &str = include_str!("github_detail.graphql");
const LINEAR_DETAIL_QUERY: &str = include_str!("linear_detail.graphql");
const GITHUB_THREAD_REPLY: &str = concat!(
    "mutation($id:ID!,$body:String!){",
    "addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$id,body:$body})",
    "{comment{id}}}",
);
const GITHUB_ADD_COMMENT: &str = concat!(
    "mutation($id:ID!,$body:String!){",
    "addComment(input:{subjectId:$id,body:$body}){commentEdge{node{id}}}}",
);
const LINEAR_CREATE_COMMENT: &str = concat!(
    "mutation($input:CommentCreateInput!){",
    "commentCreate(input:$input){success}}",
);
const GITHUB_MERGE: &str = concat!(
    "mutation($id:ID!,$head:GitObjectID!,$method:PullRequestMergeMethod!){",
    "mergePullRequest(input:{pullRequestId:$id,expectedHeadOid:$head,mergeMethod:$method})",
    "{pullRequest{id}}}",
);
const GITHUB_CONVERT_DRAFT: &str = concat!(
    "mutation($id:ID!){",
    "convertPullRequestToDraft(input:{pullRequestId:$id}){pullRequest{id}}}",
);
const GITHUB_MARK_READY: &str = concat!(
    "mutation($id:ID!){",
    "markPullRequestReadyForReview(input:{pullRequestId:$id}){pullRequest{id}}}",
);
const GITHUB_CLOSE: &str = concat!(
    "mutation($id:ID!){",
    "closePullRequest(input:{pullRequestId:$id}){pullRequest{id}}}"
);
const GITHUB_REOPEN: &str = concat!(
    "mutation($id:ID!){",
    "reopenPullRequest(input:{pullRequestId:$id}){pullRequest{id}}}",
);
const GITHUB_UPDATE_BRANCH: &str = concat!(
    "mutation($id:ID!,$head:GitObjectID!){",
    "updatePullRequestBranch(input:{pullRequestId:$id,expectedHeadOid:$head,updateMethod:MERGE})",
    "{pullRequest{id}}}",
);
const GITHUB_ENABLE_AUTO_MERGE: &str = concat!(
    "mutation($id:ID!,$head:GitObjectID!,$method:PullRequestMergeMethod!){",
    "enablePullRequestAutoMerge(input:{pullRequestId:$id,expectedHeadOid:$head,",
    "mergeMethod:$method}){pullRequest{id}}}",
);
const GITHUB_DISABLE_AUTO_MERGE: &str = concat!(
    "mutation($id:ID!){",
    "disablePullRequestAutoMerge(input:{pullRequestId:$id}){pullRequest{id}}}",
);
const GITHUB_DELETE_REF: &str =
    "mutation($ref:ID!){deleteRef(input:{refId:$ref}){clientMutationId}}";
const GITHUB_MARK_VIEWED: &str = concat!(
    "mutation($id:ID!,$path:String!){",
    "markFileAsViewed(input:{pullRequestId:$id,path:$path}){clientMutationId}}",
);
const GITHUB_UNMARK_VIEWED: &str = concat!(
    "mutation($id:ID!,$path:String!){",
    "unmarkFileAsViewed(input:{pullRequestId:$id,path:$path}){clientMutationId}}",
);
const GITHUB_SUBMIT_REVIEW: &str = concat!(
    "mutation($id:ID!,$commit:GitObjectID!,$event:PullRequestReviewEvent!,$body:String,",
    "$threads:[DraftPullRequestReviewThread]){",
    "addPullRequestReview(input:{pullRequestId:$id,commitOID:$commit,event:$event,",
    "body:$body,threads:$threads}){pullRequestReview{id}}}",
);
const GITHUB_COMMITS: &str = concat!(
    "query($id:ID!){node(id:$id){... on PullRequest{commits(last:100){totalCount ",
    "nodes{commit{oid abbreviatedOid messageHeadline committedDate ",
    "author{name user{login}} statusCheckRollup{state}}}}}}}",
);

pub fn load_detail(item: &WorkItem) -> Result<WorkDetail> {
    if item.remote_id.is_empty() {
        bail!("The task has no remote ID. Refresh Review.");
    }
    match item.source {
        WorkSource::GitHub => {
            let data = github::graphql(DETAIL_QUERY, json!({ "id": item.remote_id }))?;
            let node = data
                .get("node")
                .filter(|node| node.is_object())
                .context("The task is no longer available on GitHub.")?;
            Ok(parse_github_detail(node))
        }
        WorkSource::Linear => {
            let data = linear::authenticated_graphql(
                LINEAR_DETAIL_QUERY,
                json!({ "id": item.remote_id }),
            )?;
            let issue = data
                .get("issue")
                .filter(|issue| issue.is_object())
                .context("The task is no longer available on Linear.")?;
            let nodes = array(&issue["comments"]["nodes"]);
            let mut comments: Vec<_> = nodes
                .iter()
                .filter(|node| node["parent"]["id"].as_str().is_none())
                .map(linear_comment)
                .collect();
            for node in nodes
                .iter()
                .filter(|node| node["parent"]["id"].as_str().is_some())
            {
                let reply = linear_comment(node);
                if let Some(parent) = comments
                    .iter_mut()
                    .find(|comment| Some(comment.id.as_str()) == node["parent"]["id"].as_str())
                {
                    parent.replies.push(reply);
                } else {
                    comments.push(reply);
                }
            }
            Ok(WorkDetail {
                body: string(issue, "description"),
                comments,
                truncated: issue["comments"]["pageInfo"]["hasPreviousPage"]
                    .as_bool()
                    .unwrap_or(false),
                ..Default::default()
            })
        }
    }
}

fn github_comment(node: &Value) -> WorkComment {
    WorkComment {
        id: string(node, "id"),
        author: string(&node["author"], "login"),
        body: string(node, "body"),
        at: timestamp(node.get("createdAt").unwrap_or(&node["submittedAt"])),
        url: string(node, "url"),
        context: string(node, "state"),
        ..Default::default()
    }
}

fn linear_comment(node: &Value) -> WorkComment {
    WorkComment {
        id: string(node, "id"),
        reply_id: Some(string(node, "id")),
        author: string(&node["user"], "name"),
        body: string(node, "body"),
        at: timestamp(&node["createdAt"]),
        url: string(node, "url"),
        ..Default::default()
    }
}

fn parse_github_detail(node: &Value) -> WorkDetail {
    let flag = |key: &str| node[key].as_bool() == Some(true);
    let mut detail = WorkDetail {
        body: string(node, "body"),
        base_ref: string(node, "baseRefName"),
        head_ref: string(node, "headRefName"),
        head_oid: string(node, "headRefOid"),
        review_decision: string(node, "reviewDecision"),
        merge: MergeState {
            mergeable: string(node, "mergeable"),
            merge_state: string(node, "mergeStateStatus"),
            auto_merge: node["autoMergeRequest"]["mergeMethod"]
                .as_str()
                .map(str::to_owned),
            can_update_branch: flag("viewerCanUpdateBranch"),
            can_enable_auto_merge: flag("viewerCanEnableAutoMerge"),
            can_disable_auto_merge: flag("viewerCanDisableAutoMerge"),
            can_delete_head: flag("viewerCanDeleteHeadRef"),
            head_ref_id: string(&node["headRef"], "id"),
            head_repository: string(&node["headRepository"], "nameWithOwner"),
            cross_repository: flag("isCrossRepository"),
        },
        viewer_review_oid: string(&node["viewerLatestReview"]["commit"], "oid"),
        viewed: array(&node["files"]["nodes"])
            .iter()
            .filter_map(|file| {
                let state = match file["viewerViewedState"].as_str()? {
                    "VIEWED" => FileViewed::Viewed,
                    "DISMISSED" => FileViewed::Dismissed,
                    _ => FileViewed::Unviewed,
                };
                Some((file["path"].as_str()?.to_owned(), state))
            })
            .collect(),
        ..Default::default()
    };
    for connection in ["comments", "reviews"] {
        let nodes = github_nodes(&node[connection], &mut detail.truncated);
        detail.comments.extend(
            nodes
                .iter()
                .filter(|comment| {
                    comment["isMinimized"].as_bool() != Some(true)
                        && comment["body"]
                            .as_str()
                            .is_some_and(|body| !body.trim().is_empty())
                })
                .map(github_comment),
        );
    }
    let threads = github_nodes(&node["reviewThreads"], &mut detail.truncated);
    for thread in threads {
        let nodes = github_nodes(&thread["comments"], &mut detail.truncated);
        let mut comments = nodes
            .iter()
            .filter(|node| node["isMinimized"].as_bool() != Some(true))
            .map(github_comment);
        if let Some(mut parent) = comments.next() {
            parent.reply_id = Some(string(thread, "id"));
            parent.context = format!(
                "{}{}{}",
                string(thread, "path"),
                nodes
                    .first()
                    .and_then(|node| node["line"].as_u64().or(node["originalLine"].as_u64()))
                    .map(|n| format!(":{n}"))
                    .unwrap_or_default(),
                if thread["isResolved"].as_bool() == Some(true) {
                    " · Resolved"
                } else {
                    ""
                }
            );
            parent.replies = comments.collect();
            detail.threads.push(WorkThread {
                id: string(thread, "id"),
                path: string(thread, "path"),
                line: thread["line"].as_u64(),
                left: thread["diffSide"].as_str() == Some("LEFT"),
                outdated: thread["isOutdated"].as_bool() == Some(true),
                resolved: thread["isResolved"].as_bool() == Some(true),
                comments: std::iter::once(WorkComment {
                    replies: Vec::new(),
                    ..parent.clone()
                })
                .chain(parent.replies.iter().cloned())
                .collect(),
            });
            detail.comments.push(parent);
        }
    }
    detail.comments.sort_by_key(|comment| comment.at);
    detail
}

fn github_nodes<'a>(connection: &'a Value, truncated: &mut bool) -> &'a [Value] {
    let nodes = array(&connection["nodes"]);
    *truncated |= connection["totalCount"].as_u64().unwrap_or_default() > nodes.len() as u64;
    nodes
}

fn github_target(item: &WorkItem) -> Result<(String, u64)> {
    let path = item
        .url
        .strip_prefix("https://github.com/")
        .context("Invalid GitHub link.")?;
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 4 || !matches!(parts[2], "issues" | "pull") {
        bail!("Invalid GitHub destination.");
    }
    let repo =
        github::repository_from_remote(&format!("https://github.com/{}/{}", parts[0], parts[1]))
            .context("Invalid repository.")?;
    let number = parts[3]
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .context("Invalid task number.")?;
    Ok((repo, number))
}

pub fn load_diff(item: &WorkItem) -> Result<WorkDiff> {
    let (repo, number) = github_target(item)?;
    if item.kind != WorkKind::PullRequest {
        bail!("Only pull requests have diffs.");
    }
    let mut diff = WorkDiff::default();
    for page in 1..=5 {
        let endpoint = format!("repos/{repo}/pulls/{number}/files?per_page=100&page={page}");
        let value = github::gh(&["api", "--hostname", "github.com", &endpoint], None)?;
        let files = value
            .as_array()
            .context("GitHub did not return the pull request files.")?;
        diff.files.extend(files.iter().map(diff_file));
        if files.len() < 100 {
            return Ok(diff);
        }
    }
    diff.truncated = true;
    Ok(diff)
}

fn diff_file(file: &Value) -> WorkDiffFile {
    WorkDiffFile {
        path: string(file, "filename"),
        blob_oid: string(file, "sha"),
        removed: file["status"].as_str() == Some("removed"),
        previous_path: file["previous_filename"].as_str().map(str::to_owned),
        additions: file["additions"].as_u64().unwrap_or_default(),
        deletions: file["deletions"].as_u64().unwrap_or_default(),
        patch: string(file, "patch"),
    }
}

fn object_id(oid: &str) -> Result<&str> {
    if oid.len() != 40 || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Invalid commit. Refresh the pull request.");
    }
    Ok(oid)
}

/// The pull request's commits, oldest first, with their CI state.
pub fn load_commits(item: &WorkItem) -> Result<WorkCommits> {
    if item.kind != WorkKind::PullRequest || item.source != WorkSource::GitHub {
        bail!("Only pull requests have commits.");
    }
    let data = github::graphql(GITHUB_COMMITS, json!({ "id": item.remote_id }))?;
    parse_commits(&data["node"]["commits"])
}

fn parse_commits(connection: &Value) -> Result<WorkCommits> {
    let nodes = connection["nodes"]
        .as_array()
        .context("GitHub did not return the commits.")?;
    let commits: Vec<_> = nodes
        .iter()
        .map(|node| &node["commit"])
        .map(|commit| WorkCommit {
            oid: string(commit, "oid"),
            short_oid: string(commit, "abbreviatedOid"),
            headline: string(commit, "messageHeadline"),
            author: commit["author"]["user"]["login"]
                .as_str()
                .or(commit["author"]["name"].as_str())
                .unwrap_or_default()
                .to_owned(),
            at: timestamp(&commit["committedDate"]),
            checks: CheckRollup::from_state(
                commit["statusCheckRollup"]["state"]
                    .as_str()
                    .unwrap_or_default(),
            ),
        })
        .filter(|commit| object_id(&commit.oid).is_ok())
        .collect();
    Ok(WorkCommits {
        total: (connection["totalCount"].as_u64().unwrap_or_default() as usize).max(commits.len()),
        commits,
    })
}

/// One commit of the pull request, compared with its first parent.
pub fn load_commit_diff(item: &WorkItem, oid: &str) -> Result<WorkDiff> {
    let (repo, _) = github_target(item)?;
    let oid = object_id(oid)?;
    let mut diff = WorkDiff::default();
    for page in 1..=3 {
        let endpoint = format!("repos/{repo}/commits/{oid}?per_page=100&page={page}");
        let value = github::gh(&["api", "--hostname", "github.com", &endpoint], None)?;
        let files = value["files"]
            .as_array()
            .context("GitHub did not return the commit files.")?;
        diff.files.extend(files.iter().map(diff_file));
        if files.len() < 100 {
            return Ok(diff);
        }
    }
    diff.truncated = true;
    Ok(diff)
}

/// What changed between two heads of the pull request.
pub fn load_compare_diff(item: &WorkItem, base: &str, head: &str) -> Result<WorkDiff> {
    let (repo, _) = github_target(item)?;
    let endpoint = format!(
        "repos/{repo}/compare/{}...{}",
        object_id(base)?,
        object_id(head)?
    );
    let value = github::gh(&["api", "--hostname", "github.com", &endpoint], None)?;
    let files = value["files"]
        .as_array()
        .context("GitHub could not compare these commits. Refresh the pull request.")?;
    Ok(WorkDiff {
        truncated: files.len() >= 300,
        files: files.iter().map(diff_file).collect(),
    })
}

/// Sync a file's "Viewed" mark with GitHub, where it is per user.
pub fn set_file_viewed(item: &WorkItem, path: &str, viewed: bool) -> Result<()> {
    if item.kind != WorkKind::PullRequest || item.source != WorkSource::GitHub {
        bail!("Only pull requests track viewed files.");
    }
    github::graphql(
        if viewed {
            GITHUB_MARK_VIEWED
        } else {
            GITHUB_UNMARK_VIEWED
        },
        json!({ "id": item.remote_id, "path": path }),
    )?;
    Ok(())
}

/// Publish a review with its inline comments, pinned to the head that was read.
pub fn submit_review(
    item: &WorkItem,
    head_oid: &str,
    verdict: ReviewVerdict,
    body: &str,
    threads: &[DraftThread],
) -> Result<()> {
    if item.kind != WorkKind::PullRequest || item.source != WorkSource::GitHub {
        bail!("Only pull requests take reviews.");
    }
    if body.len() > 65_536 || threads.iter().any(|thread| thread.body.len() > 65_536) {
        bail!("Review comments must be at most 65,536 bytes.");
    }
    if verdict == ReviewVerdict::Comment && body.trim().is_empty() && threads.is_empty() {
        bail!("Write a comment before submitting the review.");
    }
    github::graphql(
        GITHUB_SUBMIT_REVIEW,
        review_variables(item, head_oid, verdict, body, threads)?,
    )?;
    Ok(())
}

fn review_variables(
    item: &WorkItem,
    head_oid: &str,
    verdict: ReviewVerdict,
    body: &str,
    threads: &[DraftThread],
) -> Result<Value> {
    Ok(json!({
        "id": item.remote_id,
        "commit": object_id(head_oid)?,
        "event": match verdict {
            ReviewVerdict::Comment => "COMMENT",
            ReviewVerdict::Approve => "APPROVE",
            ReviewVerdict::RequestChanges => "REQUEST_CHANGES",
        },
        "body": body,
        "threads": threads
            .iter()
            .map(|thread| json!({
                "path": thread.path,
                "line": thread.line,
                "side": if thread.left { "LEFT" } else { "RIGHT" },
                "body": thread.body,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// `gh pr checkout` in a local repository. It creates or updates the PR
/// branch with its upstream, including pull requests from forks.
pub fn checkout_pull_request(item: &WorkItem, directory: &std::path::Path) -> Result<()> {
    if item.kind != WorkKind::PullRequest || item.source != WorkSource::GitHub {
        bail!("Only pull requests can be checked out.");
    }
    let (repo, number) = github_target(item)?;
    let url = format!("https://github.com/{repo}/pull/{number}");
    github::gh_write(&["pr", "checkout", &url], directory)
}

pub fn load_full_file(item: &WorkItem, file: &WorkDiffFile) -> Result<String> {
    let (repo, _) = github_target(item)?;
    if file.blob_oid.len() != 40 || !file.blob_oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("This file has no available Git blob. Refresh the pull request.");
    }
    // Use the immutable blob returned with the diff, never a moving branch or local checkout.
    let endpoint = format!("repos/{repo}/git/blobs/{}", file.blob_oid);
    let data = github::gh(&["api", "--hostname", "github.com", &endpoint], None)?;
    decode_file_blob(&data)
}

fn decode_file_blob(data: &Value) -> Result<String> {
    const LIMIT: u64 = 2 * 1024 * 1024;
    if data["size"].as_u64().is_some_and(|size| size > LIMIT) {
        bail!("This file is too large to display. Open it on GitHub.");
    }
    if data["encoding"].as_str() != Some("base64") {
        bail!("GitHub did not return text content for this file.");
    }
    let content: String = data["content"]
        .as_str()
        .context("GitHub did not return the file content.")?
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content)
        .context("GitHub returned invalid file content.")?;
    if bytes.len() > LIMIT as usize {
        bail!("This file is too large to display. Open it on GitHub.");
    }
    if bytes.contains(&0) {
        bail!("This is a binary file. Open it on GitHub to view it.");
    }
    String::from_utf8(bytes).context("This file is not UTF-8 text. Open it on GitHub to view it.")
}

pub fn load_checks(item: &WorkItem) -> Result<Vec<WorkCheck>> {
    let (repo, number) = github_target(item)?;
    let data = github::gh(
        &[
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &repo,
            "--json",
            "statusCheckRollup,headRefOid",
        ],
        None,
    )?;
    parse_checks(&data)
}

fn parse_checks(data: &Value) -> Result<Vec<WorkCheck>> {
    let nodes = data["statusCheckRollup"]
        .as_array()
        .context("GitHub did not return the checks.")?;
    Ok(nodes
        .iter()
        .map(|node| WorkCheck {
            workflow: string(node, "workflowName"),
            duration_seconds: {
                let start = timestamp(&node["startedAt"]);
                let end = timestamp(&node["completedAt"]);
                (start > 0 && end >= start).then(|| end - start)
            },
            head_oid: string(data, "headRefOid"),
            name: node["name"]
                .as_str()
                .or(node["context"].as_str())
                .unwrap_or("Check")
                .into(),
            state: node["conclusion"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or(node["state"].as_str())
                .or(node["status"].as_str())
                .unwrap_or("PENDING")
                .into(),
            url: node["detailsUrl"]
                .as_str()
                .or(node["targetUrl"].as_str())
                .unwrap_or_default()
                .into(),
        })
        .collect())
}

/// Only GitHub Actions job URLs can resolve logs; third-party checks keep their link.
pub fn github_check_job(item: &WorkItem, check: &WorkCheck) -> Option<u64> {
    let (repo, _) = github_target(item).ok()?;
    let path = check
        .url
        .strip_prefix(&format!("https://github.com/{repo}/actions/runs/"))?;
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 3 || parts[1] != "job" || parts[0].parse::<u64>().ok()? == 0 {
        return None;
    }
    parts[2].parse::<u64>().ok().filter(|id| *id > 0)
}

pub fn load_check_log(item: &WorkItem, check: &WorkCheck) -> Result<String> {
    let (repo, _) = github_target(item)?;
    let job = github_check_job(item, check)
        .context("This check has no GitHub Actions job. Open its link to view the log.")?;
    let bytes = github::gh_output(
        &[
            "run",
            "view",
            "--repo",
            &repo,
            "--job",
            &job.to_string(),
            "--log",
        ],
        None,
    )?;
    let text = String::from_utf8_lossy(&bytes);
    let mut log: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(200_000)
        .collect();
    if text.chars().count() > 200_000 {
        log.push_str("\n… Log truncated. Open GitHub to view the full log.");
    }
    if log.trim().is_empty() {
        log = "The job has no logs available yet.".into();
    }
    Ok(log)
}

pub fn post_comment(item: &WorkItem, body: &str, reply: Option<&str>) -> Result<()> {
    if body.trim().is_empty() || body.len() > 65_536 {
        bail!("The comment must be between 1 and 65,536 bytes.");
    }
    match item.source {
        WorkSource::GitHub => {
            let (query, variables) = if let Some(reply) = reply {
                (GITHUB_THREAD_REPLY, json!({ "id": reply, "body": body }))
            } else {
                (
                    GITHUB_ADD_COMMENT,
                    json!({ "id": item.remote_id, "body": body }),
                )
            };
            github::graphql(query, variables)?;
        }
        WorkSource::Linear => {
            let mut input = json!({ "issueId": item.remote_id, "body": body });
            if let Some(reply) = reply {
                input["parentId"] = json!(reply);
            }
            let data =
                linear::authenticated_graphql(LINEAR_CREATE_COMMENT, json!({ "input": input }))?;
            if data["commentCreate"]["success"].as_bool() != Some(true) {
                bail!("Linear could not post the comment.");
            }
        }
    }
    Ok(())
}

pub fn run_pr_action(
    item: &WorkItem,
    action: PrAction,
    head_oid: &str,
    head_ref_id: &str,
) -> Result<()> {
    if item.source != WorkSource::GitHub || item.kind != WorkKind::PullRequest {
        bail!("This action requires a GitHub pull request.");
    }
    let (query, variables) = pr_action_request(item, action, head_oid, head_ref_id)?;
    github::graphql(query, variables)?;
    Ok(())
}

fn pr_action_request(
    item: &WorkItem,
    action: PrAction,
    head_oid: &str,
    head_ref_id: &str,
) -> Result<(&'static str, Value)> {
    let mut variables = json!({ "id": item.remote_id });
    let method = match action {
        PrAction::Squash | PrAction::AutoSquash => "SQUASH",
        PrAction::Rebase | PrAction::AutoRebase => "REBASE",
        _ => "MERGE",
    };
    if matches!(
        action,
        PrAction::Merge
            | PrAction::Squash
            | PrAction::Rebase
            | PrAction::AutoMerge
            | PrAction::AutoSquash
            | PrAction::AutoRebase
            | PrAction::UpdateBranch
    ) {
        if head_oid.is_empty() {
            bail!("Refresh the pull request before changing it.");
        }
        variables["head"] = json!(head_oid);
    }
    let query = match action {
        PrAction::Merge | PrAction::Squash | PrAction::Rebase => {
            variables["method"] = json!(method);
            GITHUB_MERGE
        }
        PrAction::AutoMerge | PrAction::AutoSquash | PrAction::AutoRebase => {
            variables["method"] = json!(method);
            GITHUB_ENABLE_AUTO_MERGE
        }
        PrAction::UpdateBranch => GITHUB_UPDATE_BRANCH,
        PrAction::DisableAutoMerge => GITHUB_DISABLE_AUTO_MERGE,
        PrAction::DeleteBranch => {
            if head_ref_id.is_empty() {
                bail!("The branch no longer exists on GitHub.");
            }
            return Ok((GITHUB_DELETE_REF, json!({ "ref": head_ref_id })));
        }
        PrAction::Draft => GITHUB_CONVERT_DRAFT,
        PrAction::Ready => GITHUB_MARK_READY,
        PrAction::Close => GITHUB_CLOSE,
        PrAction::Reopen => GITHUB_REOPEN,
    };
    Ok((query, variables))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_keep_workflows_and_only_completed_durations() {
        let checks = parse_checks(&json!({
            "headRefOid": "abc",
            "statusCheckRollup": [
                {
                    "name": "Lint",
                    "conclusion": "SUCCESS",
                    "workflowName": "CI",
                    "startedAt": "2026-09-20T01:00:00Z",
                    "completedAt": "2026-09-20T01:03:35Z"
                },
                {
                    "name": "Tests",
                    "conclusion": "",
                    "status": "IN_PROGRESS",
                    "startedAt": "2026-09-20T01:00:00Z"
                },
                {
                    "context": "Deploy",
                    "state": "SUCCESS",
                    "targetUrl": "https://ci.example.com/job"
                },
                {
                    "name": "Clock skew",
                    "conclusion": "SUCCESS",
                    "startedAt": "2026-09-20T01:00:10Z",
                    "completedAt": "2026-09-20T01:00:00Z"
                }
            ]
        }))
        .unwrap();
        assert_eq!(checks[0].workflow, "CI");
        assert_eq!(checks[0].duration_seconds, Some(215));
        assert!(checks[0].passed());
        assert_eq!(checks[1].state, "IN_PROGRESS");
        assert!(checks[1].duration_seconds.is_none());
        assert_eq!(checks[2].name, "Deploy");
        assert!(checks[2].workflow.is_empty());
        assert!(checks[2].duration_seconds.is_none());
        assert!(checks[3].duration_seconds.is_none());
    }

    #[test]
    fn full_files_decode_text_and_reject_binary_invalid_or_oversized_content() {
        let text = "fn main() {}\n";
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        assert_eq!(
            decode_file_blob(&json!({
                "encoding": "base64",
                "content": format!("{encoded}\n"),
                "size": text.len()
            }))
            .unwrap(),
            text
        );
        for value in [
            json!({ "encoding": "base64", "content": "AA==", "size": 1 }),
            json!({ "encoding": "base64", "content": "/w==", "size": 1 }),
            json!({ "encoding": "base64", "content": "invalid!" }),
            json!({ "encoding": "base64", "content": "", "size": 2097153 }),
            json!({ "encoding": "base64" }),
        ] {
            assert!(decode_file_blob(&value).is_err());
        }
    }

    #[test]
    fn review_threads_keep_replies_paths_and_truncation() {
        let detail = parse_github_detail(&json!({
            "body": "Summary",
            "headRefOid": "abc",
            "comments": {
                "totalCount": 2,
                "nodes": [{
                    "id": "1",
                    "body": "Comment",
                    "author": { "login": "ana" },
                    "createdAt": "2026-09-20T01:00:00Z"
                }]
            },
            "reviewThreads": {
                "nodes": [{
                    "id": "thread",
                    "path": "src/main.rs",
                    "isResolved": true,
                    "comments": {
                        "totalCount": 2,
                        "nodes": [
                            { "id": "2", "body": "Fix", "line": 42 },
                            { "id": "3", "body": "Done" }
                        ]
                    }
                }]
            }
        }));
        assert!(detail.truncated);
        let thread = detail
            .comments
            .iter()
            .find(|comment| comment.reply_id.is_some())
            .unwrap();
        assert_eq!(thread.replies.len(), 1);
        assert_eq!(thread.context, "src/main.rs:42 · Resolved");
        assert_eq!(thread.reply_id.as_deref(), Some("thread"));
    }

    #[test]
    fn pull_request_detail_keeps_merge_state_viewed_files_and_anchored_threads() {
        let detail = parse_github_detail(&json!({
            "headRefOid": "abc",
            "mergeable": "CONFLICTING",
            "mergeStateStatus": "DIRTY",
            "viewerCanUpdateBranch": true,
            "viewerCanDeleteHeadRef": true,
            "autoMergeRequest": { "mergeMethod": "SQUASH" },
            "headRef": { "id": "REF_1" },
            "viewerLatestReview": { "commit": { "oid": "def" } },
            "files": { "nodes": [
                { "path": "a.rs", "viewerViewedState": "VIEWED" },
                { "path": "b.rs", "viewerViewedState": "DISMISSED" },
                { "path": "c.rs", "viewerViewedState": "UNVIEWED" }
            ] },
            "reviewThreads": { "nodes": [{
                "id": "T1",
                "path": "a.rs",
                "line": 7,
                "diffSide": "LEFT",
                "isOutdated": false,
                "comments": { "nodes": [
                    { "id": "1", "body": "Why?", "author": { "login": "ana" } },
                    { "id": "2", "body": "Because", "author": { "login": "bo" } }
                ] }
            }] }
        }));
        assert!(detail.merge.conflicts() && !detail.merge.behind());
        assert!(detail.merge.can_update_branch && detail.merge.can_delete_head);
        assert_eq!(detail.merge.auto_merge.as_deref(), Some("SQUASH"));
        assert_eq!(detail.merge.head_ref_id, "REF_1");
        assert_eq!(detail.viewer_review_oid, "def");
        assert_eq!(detail.viewed["a.rs"], FileViewed::Viewed);
        assert_eq!(detail.viewed["b.rs"], FileViewed::Dismissed);
        assert_eq!(detail.viewed["c.rs"], FileViewed::Unviewed);
        let thread = &detail.threads[0];
        assert_eq!((thread.line, thread.left), (Some(7), true));
        assert_eq!(thread.comments.len(), 2);
        assert!(thread.comments[0].replies.is_empty());
        // The Summary keeps showing the same thread as one conversation.
        assert_eq!(detail.comments[0].replies.len(), 1);
    }

    #[test]
    fn commits_keep_order_checks_and_reject_invalid_objects() {
        let commits = parse_commits(&json!({
            "totalCount": 120,
            "nodes": [
                { "commit": {
                    "oid": "a".repeat(40), "abbreviatedOid": "aaaaaaa",
                    "messageHeadline": "First", "committedDate": "2026-09-20T01:00:00Z",
                    "author": { "name": "Ana", "user": { "login": "ana" } },
                    "statusCheckRollup": { "state": "FAILURE" }
                } },
                { "commit": { "oid": "not-an-oid", "messageHeadline": "Bad" } },
                { "commit": {
                    "oid": "b".repeat(40), "messageHeadline": "Second",
                    "author": { "name": "Bo", "user": null }
                } }
            ]
        }))
        .unwrap();
        assert_eq!(commits.total, 120);
        assert_eq!(
            commits
                .commits
                .iter()
                .map(|commit| commit.headline.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second"]
        );
        assert_eq!(commits.commits[0].author, "ana");
        assert_eq!(commits.commits[0].checks, CheckRollup::Failing);
        assert_eq!(commits.commits[1].author, "Bo");
        assert!(parse_commits(&json!({})).is_err());
    }

    #[test]
    fn reviews_and_pr_actions_name_their_exact_target() {
        let mut item = crate::domain::work_items::pull_fixture();
        item.remote_id = "PR_1".into();
        let head = "c".repeat(40);
        let variables = review_variables(
            &item,
            &head,
            ReviewVerdict::RequestChanges,
            "Summary",
            &[DraftThread {
                path: "src/lib.rs".into(),
                line: 4,
                left: true,
                body: "Keep this".into(),
            }],
        )
        .unwrap();
        assert_eq!(variables["event"], "REQUEST_CHANGES");
        assert_eq!(variables["commit"], head.as_str());
        assert_eq!(variables["threads"][0]["side"], "LEFT");
        assert!(review_variables(&item, "abc", ReviewVerdict::Comment, "", &[]).is_err());

        let (query, variables) = pr_action_request(&item, PrAction::AutoSquash, &head, "").unwrap();
        assert_eq!(query, GITHUB_ENABLE_AUTO_MERGE);
        assert_eq!(variables["method"], "SQUASH");
        assert_eq!(variables["head"], head.as_str());
        let (query, variables) =
            pr_action_request(&item, PrAction::DeleteBranch, &head, "REF_1").unwrap();
        assert_eq!(query, GITHUB_DELETE_REF);
        assert_eq!(variables, json!({ "ref": "REF_1" }));
        assert!(pr_action_request(&item, PrAction::DeleteBranch, &head, "").is_err());
        assert!(pr_action_request(&item, PrAction::UpdateBranch, "", "").is_err());
        let (query, _) = pr_action_request(&item, PrAction::DisableAutoMerge, "", "").unwrap();
        assert_eq!(query, GITHUB_DISABLE_AUTO_MERGE);
    }

    #[test]
    fn check_logs_are_scoped_to_the_pr_repository() {
        let item = crate::domain::work_items::fixture();
        let mut check = WorkCheck {
            name: "CI".into(),
            state: "FAILURE".into(),
            head_oid: "abc".into(),
            url: "https://github.com/demo/app/actions/runs/12/job/34".into(),
            ..Default::default()
        };
        assert_eq!(github_check_job(&item, &check), Some(34));
        for url in [
            "https://github.com/other/app/actions/runs/12/job/34",
            "https://github.com/demo/app/actions/runs/12/job/34?redirect=evil",
            "https://ci.example.com/34",
        ] {
            check.url = url.into();
            assert_eq!(github_check_job(&item, &check), None);
        }
    }

    #[test]
    fn remote_target_cannot_be_redirected_by_repo_or_path() {
        let mut item = crate::domain::work_items::fixture();
        item.repository = "unrelated/checkout".into();
        assert_eq!(github_target(&item).unwrap(), ("demo/app".into(), 42));
        for url in [
            "https://github.com.evil/demo/app/pull/42",
            "https://github.com/demo/app/pull/42?x=1",
            "https://github.com/demo/app/../42",
        ] {
            item.url = url.into();
            assert!(github_target(&item).is_err());
        }
    }
}

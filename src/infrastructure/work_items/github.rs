use std::collections::BTreeMap;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{WorkItemsPage, WorkQuery, bounded_output, graphql_data, string, timestamp};
use crate::domain::work_items::{WorkItem, WorkKind, WorkSource, WorkStatus};

const QUERY: &str = r#"query($search: String!) {
  search(query: $search, type: ISSUE, first: 100) {
    pageInfo { hasNextPage }
    nodes {
      __typename
      ... on Issue {
        id number title url body createdAt updatedAt state stateReason author { login }
        repository { nameWithOwner } labels(first: 20) { nodes { name } }
        assignees(first: 20) { nodes { login } }
      }
      ... on PullRequest {
        id number title url body createdAt updatedAt state isDraft merged author { login }
        repository { nameWithOwner } labels(first: 20) { nodes { name } }
        assignees(first: 20) { nodes { login } }
      }
    }
  }
}"#;

pub(super) fn gh(arguments: &[&str], input: Option<Vec<u8>>) -> Result<Value> {
    let bytes = gh_output(arguments, input)?;
    serde_json::from_slice(&bytes).context("GitHub returned an invalid response.")
}

pub(super) fn gh_output(arguments: &[&str], input: Option<Vec<u8>>) -> Result<Vec<u8>> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/zsh".into());
    let bytes = bounded_output(
        Command::new(shell)
            .args(["-l", "-c", "exec gh \"$@\"", "vibra-inbox"])
            .args(arguments)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_PAGER", "cat")
            .env("GH_HOST", "github.com"),
        input,
    )
    .context("GitHub: install gh and sign in with gh auth login.")?;
    Ok(bytes)
}

pub(super) fn list(query: &WorkQuery) -> Result<WorkItemsPage> {
    let viewer = gh(&["api", "--hostname", "github.com", "user"], None)?;
    let login = viewer["login"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("GitHub did not return the active account.")?;
    let mut repositories = BTreeMap::new();
    for project in &query.projects {
        if let Ok(bytes) = bounded_output(
            Command::new("/usr/bin/git")
                .args(["remote", "-v"])
                .current_dir(&project.root),
            None,
        ) {
            for line in String::from_utf8_lossy(&bytes).lines() {
                if let Some(repo) = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(repository_from_remote)
                {
                    repositories.entry(repo).or_insert(project.id);
                }
            }
        }
    }
    if repositories.is_empty() {
        return Ok(WorkItemsPage {
            connected: true,
            warning: Some(
                "Add a project with a GitHub remote to see its issues and pull requests.".into(),
            ),
            ..Default::default()
        });
    }
    // MonoCode scopes both assigned and unassigned views to local project remotes.
    // Each repository/kind gets its own quota, so active PRs cannot crowd out issues.
    let searches: Vec<_> = repositories
        .keys()
        .flat_map(|repo| {
            [WorkKind::Issue, WorkKind::PullRequest]
                .into_iter()
                .filter_map(|kind| {
                    if kind == WorkKind::Issue
                        && matches!(query.status, Some(WorkStatus::Draft | WorkStatus::Merged))
                    {
                        return None;
                    }
                    Some((repo.clone(), kind, search_query(repo, kind, query, login)))
                })
        })
        .collect();
    let mut page = WorkItemsPage {
        connected: true,
        ..Default::default()
    };
    let mut failed = Vec::new();
    for chunk in searches.chunks(4) {
        let results = std::thread::scope(|scope| {
            let jobs: Vec<_> = chunk
                .iter()
                .map(|(repo, kind, search)| {
                    let repositories = &repositories;
                    scope.spawn(move || {
                        let result =
                            fetch_search(search).and_then(|data| parse_page(&data, repositories));
                        (repo, kind, result)
                    })
                })
                .collect();
            jobs.into_iter().map(|job| job.join()).collect::<Vec<_>>()
        });
        for result in results {
            let (repo, kind, result) =
                result.map_err(|_| anyhow::anyhow!("Could not complete the GitHub query."))?;
            match result {
                Ok(mut batch) => {
                    page.items.append(&mut batch.items);
                    page.truncated |= batch.truncated;
                }
                Err(_) => {
                    page.failed_scopes.push((repo.clone(), *kind));
                    failed.push(format!(
                        "{repo} ({})",
                        if *kind == WorkKind::Issue {
                            "issues"
                        } else {
                            "PRs"
                        }
                    ));
                }
            }
        }
    }
    if !failed.is_empty() {
        page.warning = Some(format!(
            "Could not query: {}. Check access and refresh again.",
            failed.join(", ")
        ));
    }
    Ok(page)
}

fn search_query(repo: &str, kind: WorkKind, query: &WorkQuery, login: &str) -> String {
    let status = match query.status {
        Some(WorkStatus::Open) => "is:open",
        Some(WorkStatus::Draft) => "is:open draft:true",
        Some(WorkStatus::Closed) => "is:closed -is:merged",
        Some(WorkStatus::Merged) => "is:merged",
        None => "",
    };
    let assigned = if query.assigned_to_me {
        format!("assignee:{login}")
    } else {
        String::new()
    };
    format!(
        "repo:{repo} is:{} {assigned} {status} sort:updated-desc",
        if kind == WorkKind::Issue {
            "issue"
        } else {
            "pr"
        }
    )
}

fn fetch_search(search: &str) -> Result<Value> {
    graphql_data(gh(
        &["api", "--hostname", "github.com", "graphql", "--input", "-"],
        Some(serde_json::to_vec(
            &json!({"query": QUERY, "variables": {"search": search}}),
        )?),
    )?)
}

pub(super) fn repository_from_remote(remote: &str) -> Option<String> {
    let path = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.strip_prefix("https://github.com/"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    if [owner, repo].iter().any(|part| {
        part.is_empty()
            || !part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    }) {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn parse_page(data: &Value, repositories: &BTreeMap<String, uuid::Uuid>) -> Result<WorkItemsPage> {
    let nodes = data["search"]["nodes"]
        .as_array()
        .context("GitHub did not return the tasks.")?;
    let mut items = Vec::new();
    for node in nodes {
        let kind = match node["__typename"].as_str() {
            Some("Issue") => WorkKind::Issue,
            Some("PullRequest") => WorkKind::PullRequest,
            _ => continue,
        };
        let url = string(node, "url");
        if !url.starts_with("https://github.com/") || node["number"].as_u64().is_none() {
            bail!("GitHub returned an invalid task.");
        }
        let repository = string(&node["repository"], "nameWithOwner");
        let status = if node["merged"].as_bool() == Some(true) {
            WorkStatus::Merged
        } else if node["state"].as_str() == Some("CLOSED") {
            WorkStatus::Closed
        } else if node["isDraft"].as_bool() == Some(true) {
            WorkStatus::Draft
        } else {
            WorkStatus::Open
        };
        items.push(WorkItem {
            remote_id: string(node, "id"),
            created_at: timestamp(&node["createdAt"]),
            completed: node["stateReason"].as_str() == Some("COMPLETED"),
            group: String::new(),
            source: WorkSource::GitHub,
            kind,
            reference: format!("#{}", node["number"]),
            title: string(node, "title"),
            url,
            body: string(node, "body"),
            project_id: repositories
                .iter()
                .find(|(repo, _)| repo.eq_ignore_ascii_case(&repository))
                .map(|(_, id)| *id),
            repository,
            status,
            state_label: status.label().into(),
            author: string(&node["author"], "login"),
            assignees: names(&node["assignees"]["nodes"], "login"),
            labels: names(&node["labels"]["nodes"], "name"),
            updated_at: timestamp(&node["updatedAt"]),
        });
    }
    Ok(WorkItemsPage {
        items,
        truncated: data["search"]["pageInfo"]["hasNextPage"]
            .as_bool()
            .unwrap_or(false),
        connected: true,
        warning: None,
        failed_scopes: Vec::new(),
    })
}

fn names(value: &Value, key: &str) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v[key].as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_inbox_maps_repositories_and_distinguishes_pr_states() {
        let project = uuid::Uuid::new_v4();
        let repositories = BTreeMap::from([("demo/app".into(), project)]);
        let mut issue = json!({"__typename":"Issue", "number":42,"title":"Fix", "url":"https://github.com/demo/app/issues/42","body":"Description", "state":"OPEN", "repository":{"nameWithOwner":"Demo/App"},"updatedAt":"2026-09-01T00:00:00Z", "author":null, "assignees":{"nodes":[{"login":"ana"}]},"labels":{"nodes":[{"name":"bug"}]}});
        let mut draft = issue.clone();
        draft["__typename"] = json!("PullRequest");
        draft["isDraft"] = json!(true);
        let mut merged = draft.clone();
        merged["merged"] = json!(true);
        merged["state"] = json!("MERGED");
        issue["state"] = json!("CLOSED");
        let page = parse_page(
            &json!({"search":{"nodes":[issue,draft,merged],"pageInfo":{"hasNextPage":true}}}),
            &repositories,
        )
        .unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.status)
                .collect::<Vec<_>>(),
            [WorkStatus::Closed, WorkStatus::Draft, WorkStatus::Merged]
        );
        assert!(
            page.items
                .iter()
                .all(|item| item.project_id == Some(project))
        );
        assert_eq!(page.items[0].assignees, ["ana"]);
        assert_eq!(page.items[0].labels, ["bug"]);
        assert!(page.truncated && page.connected);
        assert!(parse_page(&json!({}), &repositories).is_err());
    }

    #[test]
    fn assigned_search_stays_scoped_to_each_project_and_kind() {
        let mut query = WorkQuery {
            projects: vec![],
            assigned_to_me: true,
            status: None,
        };
        assert_eq!(
            search_query("demo/app", WorkKind::Issue, &query, "ana"),
            "repo:demo/app is:issue assignee:ana  sort:updated-desc"
        );
        query.status = Some(WorkStatus::Closed);
        let search = search_query("demo/other", WorkKind::PullRequest, &query, "ana");
        assert!(search.contains("repo:demo/other is:pr assignee:ana is:closed -is:merged"));
    }

    #[test]
    fn github_inbox_remote_parsing_rejects_foreign_hosts_and_search_injection() {
        for remote in [
            "git@github.com:demo/app.git",
            "https://github.com/demo/app.git",
            "ssh://git@github.com/demo/app.git/",
        ] {
            assert_eq!(repository_from_remote(remote).as_deref(), Some("demo/app"));
        }
        for remote in [
            "https://github.com.evil/demo/app",
            "https://evil/demo/app",
            "git@github.com:demo/app is:public",
            "https://github.com/demo/app/extra",
        ] {
            assert_eq!(repository_from_remote(remote), None);
        }
    }
}

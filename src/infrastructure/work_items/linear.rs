use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{WorkItemsPage, WorkQuery, bounded_output, graphql_data, string, timestamp};
use crate::domain::work_items::{WorkItem, WorkKind, WorkSource, WorkStatus};
use crate::infrastructure::paths::{application_support_directory, atomic_write};

const QUERY: &str = r#"query($filter: IssueFilter) {
  issues(first: 100, orderBy: updatedAt, filter: $filter) {
    pageInfo { hasNextPage }
    nodes {
      id identifier title url description createdAt updatedAt
      state { name type } team { name } project { name }
      creator { name } assignee { name }
      labels(first: 20) { nodes { name } }
    }
  }
}"#;

fn token_path() -> Result<PathBuf> {
    Ok(application_support_directory()
        .context("Could not find the Vibra folder.")?
        .join("linear-token"))
}

pub(super) fn authenticated_graphql(query: &str, variables: Value) -> Result<Value> {
    let mut token = String::new();
    fs::File::open(token_path()?)
        .context("Connect Linear to query the task.")?
        .take(4097)
        .read_to_string(&mut token)
        .context("Could not read the Linear key.")?;
    graphql(token.trim(), query, variables)
}

pub fn linear_connected() -> bool {
    token_path().is_ok_and(|path| path.is_file())
}

pub fn connect_linear(token: &str) -> Result<()> {
    let token = token.trim();
    validate_token(token)?;
    let data = graphql(token, "query { viewer { id } }", json!({}))?;
    if data["viewer"]["id"].as_str().is_none() {
        bail!("Linear could not verify the key.");
    }
    atomic_write(&token_path()?, token.as_bytes()).context("Could not save the Linear key.")
}

pub fn disconnect_linear() -> Result<()> {
    match fs::remove_file(token_path()?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("Could not delete the Linear key."),
    }
}

fn validate_token(token: &str) -> Result<()> {
    if token.is_empty()
        || token.len() > 4096
        || token.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        bail!("Copy a valid Linear personal API key.");
    }
    Ok(())
}

pub(super) fn graphql(token: &str, query: &str, variables: Value) -> Result<Value> {
    validate_token(token)?;
    // curl config receives both the credential and body over stdin. Never
    // follow redirects or include a server's error body in user-facing errors.
    let body = serde_json::to_string(&json!({"query": query, "variables": variables}))?;
    let config = format!(
        "header = {}\nheader = \"Content-Type: application/json\"\ndata = {}\n",
        serde_json::to_string(&format!("Authorization: {token}"))?,
        serde_json::to_string(&body)?
    );
    let bytes = bounded_output(
        Command::new("/usr/bin/curl").args([
            "--disable",
            "--silent",
            "--fail",
            "--proto",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            "20",
            "--max-filesize",
            "4194304",
            "--config",
            "-",
            "https://api.linear.app/graphql",
        ]),
        Some(config.into_bytes()),
    )
    .context("Linear: could not query the API. Check your key and connection.")?;
    graphql_data(serde_json::from_slice(&bytes).context("Linear returned an invalid response.")?)
}

pub(super) fn list(query: &WorkQuery) -> Result<WorkItemsPage> {
    let file = match fs::File::open(token_path()?) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(WorkItemsPage::default()),
        Err(_) => bail!("Could not read the Linear key."),
    };
    let mut token = String::new();
    file.take(4097)
        .read_to_string(&mut token)
        .context("Could not read the Linear key.")?;
    let mut filter = json!({});
    if query.assigned_to_me {
        filter["assignee"] = json!({"isMe": {"eq": true}});
    }
    match query.status {
        Some(WorkStatus::Open | WorkStatus::Draft) => {
            filter["state"] = json!({"type": {"nin": ["completed", "canceled"]}})
        }
        Some(WorkStatus::Closed | WorkStatus::Merged) => {
            filter["state"] = json!({"type": {"in": ["completed", "canceled"]}})
        }
        None => {}
    }
    parse_page(&graphql(&token, QUERY, json!({"filter": filter}))?)
}

fn parse_page(data: &Value) -> Result<WorkItemsPage> {
    let nodes = data["issues"]["nodes"]
        .as_array()
        .context("Linear did not return the tasks.")?;
    let mut items = Vec::new();
    for node in nodes {
        let url = string(node, "url");
        if !url.starts_with("https://linear.app/") || node["identifier"].as_str().is_none() {
            bail!("Linear returned an invalid task.");
        }
        let team = string(&node["team"], "name");
        let project = string(&node["project"], "name");
        let closed = matches!(
            node["state"]["type"].as_str(),
            Some("completed" | "canceled")
        );
        items.push(WorkItem {
            remote_id: string(node, "id"),
            created_at: timestamp(&node["createdAt"]),
            completed: node["state"]["type"].as_str() == Some("completed"),
            group: if project.is_empty() {
                team.clone()
            } else {
                format!("{team} · {project}")
            },
            source: WorkSource::Linear,
            kind: WorkKind::Issue,
            reference: string(node, "identifier"),
            title: string(node, "title"),
            url,
            body: string(node, "description"),
            repository: if project.is_empty() {
                team
            } else {
                format!("{team} · {project}")
            },
            project_id: None,
            status: if closed {
                WorkStatus::Closed
            } else {
                WorkStatus::Open
            },
            state_label: string(&node["state"], "name"),
            author: string(&node["creator"], "name"),
            assignees: node["assignee"]["name"]
                .as_str()
                .map(str::to_owned)
                .into_iter()
                .collect(),
            labels: node["labels"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|label| label["name"].as_str().map(str::to_owned))
                .collect(),
            updated_at: timestamp(&node["updatedAt"]),
        });
    }
    Ok(WorkItemsPage {
        items,
        truncated: data["issues"]["pageInfo"]["hasNextPage"]
            .as_bool()
            .unwrap_or(false),
        connected: true,
        warning: None,
        failed_scopes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_inbox_parses_unassigned_tasks_and_completed_states() {
        let node = json!({"identifier":"ENG-12", "title":"Title", "url":"https://linear.app/demo/issue/ENG-12", "description":"Full description", "state":{"name":"Done","type":"completed"}, "team":{"name":"Engineering"},"project":{"name":"App"},"assignee":null,"creator":{"name":"Ana"},"updatedAt":"2026-09-01T00:00:00Z", "labels":{"nodes":[]}});
        let page = parse_page(&json!({"issues":{"nodes":[node],"pageInfo":{"hasNextPage":false}}}))
            .unwrap();
        assert_eq!(page.items[0].status, WorkStatus::Closed);
        assert_eq!(page.items[0].repository, "Engineering · App");
        assert_eq!(page.items[0].body, "Full description");
        assert!(page.items[0].assignees.is_empty());
        assert!(parse_page(&json!({"issues":null})).is_err());
    }

    #[test]
    fn linear_inbox_rejects_header_injection_without_echoing_secrets() {
        for token in ["", "secret\nheader: evil", "secret\rnext", "two words"] {
            let error = validate_token(token).unwrap_err().to_string();
            assert!(!error.contains("secret"));
        }
        assert!(validate_token("lin_api_test-only").is_ok());
        let error =
            graphql_data(json!({"errors":[{"message":"secret-token"}],"data":null})).unwrap_err();
        assert!(!error.to_string().contains("secret-token"));
    }
}

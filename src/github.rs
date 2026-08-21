use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{Ok, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct GithubPathEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    path_type: String,
    sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct GithubRepoResponse {
    truncated: bool,
    sha: String,
    url: String,
    tree: Vec<GithubPathEntry>,
}

fn github_token() -> Result<String> {
    std::env::var("GITHUB_TOKEN").map_err(|_| anyhow::anyhow!("github credentials unavailable"))
}

static CLIENT: OnceLock<Client> = OnceLock::new();
static CACHE: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();

const REQUEST_TIMEOUT_SECONDS: u64 = 30;
const CACHE_SECONDS: u64 = 300;

fn github_client() -> Result<&'static Client> {
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }

    let client = Client::builder()
        .user_agent("HackClubPyterm (brendan@hackclub.com)")
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
        .build()?;

    Ok(CLIENT.get_or_init(|| client))
}

pub async fn send_github_request(url: &str) -> Result<String> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Some((data, stored_at)) = cache.lock().await.get(url) {
        if Instant::now().duration_since(*stored_at).as_secs() < CACHE_SECONDS {
            return Ok(data.clone());
        }
    }

    let output = github_client()?
        .get(url)
        .bearer_auth(github_token()?)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let mut handle = cache.lock().await;
    handle.retain(|_, x| Instant::now().duration_since(x.1).as_secs() < CACHE_SECONDS);
    handle.insert(url.to_string(), (output.clone(), Instant::now()));

    Ok(output)
}

pub async fn get_repo_files(user: &String, repo: &String) -> Result<Vec<String>> {
    let url = format!("https://api.github.com/repos/{user}/{repo}/git/trees/HEAD?recursive=1");
    let response: GithubRepoResponse = serde_json::from_str(&send_github_request(&url).await?)?;

    Ok(response.tree.iter().map(|x| x.path.clone()).collect())
}

pub async fn get_file_in_repo(user: &String, repo: &String, file_path: &String) -> Result<String> {
    let url = format!(
        "https://raw.githubusercontent.com/{user}/{repo}/HEAD/{}",
        file_path.trim_start_matches("/")
    );
    Ok(send_github_request(&url).await?)
}

pub async fn get_python_code(user: &String, repo: &String) -> Result<String> {
    let repo_python_file = format!("{repo}.py");
    let files_as_priority = [
        "game.py",
        "main.py",
        "folktale.py",
        "folktale_game.py",
        repo_python_file.as_str(),
    ];
    let files = get_repo_files(user, repo).await?;
    let mut path = None;
    for query in files_as_priority {
        let query_string = query.to_string();
        if files.contains(&query_string) {
            path = Some(query_string);
            break;
        }
    }

    if path.is_none() {
        path = files.iter().find(|x| x.ends_with(".py")).cloned();
    }

    if path.is_none() {
        return Ok("No Python files in directory".to_string());
    }

    get_file_in_repo(user, repo, &path.unwrap()).await
}

use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use reqwest::{Client, Response, Url};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct GithubRepoInfo {
    private: bool,
}

fn github_token() -> Result<String> {
    std::env::var("GITHUB_TOKEN").map_err(|_| anyhow::anyhow!("github credentials unavailable"))
}

// Client errors are cached too so repeated bad lookups don't spend API quota.
type CacheEntry = (std::result::Result<String, String>, Instant);

static CLIENT: OnceLock<Client> = OnceLock::new();
static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

const REQUEST_TIMEOUT_SECONDS: u64 = 30;
const CACHE_SECONDS: u64 = 300;
const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;

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

async fn read_limited(mut response: Response) -> Result<std::result::Result<String, String>> {
    let too_large = Err(format!("response larger than {MAX_RESPONSE_BYTES} bytes"));
    if response
        .content_length()
        .is_some_and(|x| x > MAX_RESPONSE_BYTES as u64)
    {
        return Ok(too_large);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Ok(too_large);
        }
        body.extend_from_slice(&chunk);
    }

    Ok(Ok(String::from_utf8_lossy(&body).into_owned()))
}

pub async fn send_github_request(url: Url) -> Result<String> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = url.to_string();

    if let Some((data, stored_at)) = cache.lock().await.get(&key) {
        if Instant::now().duration_since(*stored_at).as_secs() < CACHE_SECONDS {
            return data.clone().map_err(|e| anyhow!(e));
        }
    }

    let response = github_client()?
        .get(url)
        .bearer_auth(github_token()?)
        .send()
        .await?;
    let output = if response.status().is_client_error() {
        Err(format!("github returned {}", response.status()))
    } else {
        read_limited(response.error_for_status()?).await?
    };

    let entry_size =
        |k: &String, x: &CacheEntry| k.len() + x.0.as_ref().map_or_else(String::len, String::len);

    let mut handle = cache.lock().await;
    handle.retain(|_, x| Instant::now().duration_since(x.1).as_secs() < CACHE_SECONDS);
    let new_entry = (output.clone(), Instant::now());
    let mut total =
        entry_size(&key, &new_entry) + handle.iter().map(|(k, x)| entry_size(k, x)).sum::<usize>();
    while total > MAX_CACHE_BYTES {
        let Some(oldest) = handle
            .iter()
            .min_by_key(|(_, x)| x.1)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        if let Some(removed) = handle.remove(&oldest) {
            total -= entry_size(&oldest, &removed);
        }
    }
    handle.insert(key, new_entry);

    output.map_err(|e| anyhow!(e))
}

fn github_url(base: &str, segments: &[&str]) -> Result<Url> {
    let mut url = Url::parse(base)?;
    url.path_segments_mut()
        .map_err(|_| anyhow!("invalid base url"))?
        .extend(segments);
    Ok(url)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

// The token may be able to see private repos, so only public ones are served.
async fn ensure_public(user: &str, repo: &str) -> Result<()> {
    let url = github_url("https://api.github.com/repos", &[user, repo])?;
    let info: GithubRepoInfo = serde_json::from_str(&send_github_request(url).await?)?;
    if info.private {
        bail!("{user}/{repo} is private");
    }
    Ok(())
}

pub async fn get_repo_files(user: &String, repo: &String) -> Result<Vec<String>> {
    let mut url = github_url(
        "https://api.github.com/repos",
        &[user, repo, "git", "trees", "HEAD"],
    )?;
    url.set_query(Some("recursive=1"));
    let response: GithubRepoResponse = serde_json::from_str(&send_github_request(url).await?)?;

    Ok(response.tree.iter().map(|x| x.path.clone()).collect())
}

pub async fn get_file_in_repo(user: &String, repo: &String, file_path: &String) -> Result<String> {
    let mut segments = vec![user.as_str(), repo.as_str(), "HEAD"];
    segments.extend(file_path.split('/').filter(|x| !x.is_empty()));
    let url = github_url("https://raw.githubusercontent.com", &segments)?;
    send_github_request(url).await
}

pub async fn get_python_code(user: &String, repo: &String, file: Option<&str>) -> Result<String> {
    if !valid_name(user) || !valid_name(repo) {
        bail!("invalid repository {user}/{repo}");
    }
    ensure_public(user, repo).await?;

    if let Some(file) = file {
        if file.split('/').any(|x| x == ".." || x == ".") {
            return Ok("Invalid file path".to_string());
        }
        return get_file_in_repo(user, repo, &file.to_string()).await;
    }

    let repo_python_file = format!("{repo}.py");
    let files_as_priority = [
        "game.py",
        "main.py",
        "folktale.py",
        "folktale_game.py",
        "folktale-game.py",
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

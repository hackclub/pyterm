use anyhow::{Ok, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubPathEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    path_type: String,
    sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubRepoResponse {
    truncated: bool,
    sha: String,
    url: String,
    tree: Vec<GithubPathEntry>,
}

pub async fn get_repo_files(user: &String, repo: &String) -> Result<Vec<String>> {
    let url = format!("https://api.github.com/repos/{user}/{repo}/git/trees/HEAD?recursive=1");
    let client = reqwest::Client::builder()
        .user_agent("HackClubPyterm (brendan@hackclub.com)")
        .build()?;
    let response: GithubRepoResponse = client.get(url).send().await?.json().await?;

    Ok(response.tree.iter().map(|x| x.path.clone()).collect())
}

pub async fn get_file_in_repo(user: &String, repo: &String, file_path: &String) -> Result<String> {
    let url = format!(
        "https://raw.githubusercontent.com/{user}/{repo}/HEAD/{}",
        file_path.trim_start_matches("/")
    );

    Ok(reqwest::get(url).await?.text().await?)
}

pub async fn get_python_code(user: &String, repo: &String) -> Result<String> {
    let files_as_priority = ["game.py", "main.py", "folktale.py", "folktale_game.py"];
    let files = get_repo_files(user, repo).await?;
    let mut path = None;
    for query in files_as_priority {
        let query_string = query.to_string();
        if files.contains(&query_string) {
            path = Some(query_string)
        }
    }
    if path.is_none() {
        path = files.iter().find(|x| x.contains(".py")).cloned();
    }
    get_file_in_repo(user, repo, &path.unwrap()).await
}

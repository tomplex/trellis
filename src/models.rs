// trellis/src/models.rs

#[derive(Debug, Clone)]
pub struct Repo {
    pub id: Option<i64>,
    pub path: String,
    pub name: String,
    pub default_branch: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: Option<i64>,
    pub name: String,
    pub repo_id: i64,
    pub base_branch: String,
    pub created_at: String,
    pub last_selected_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub id: Option<i64>,
    pub session_id: Option<i64>,
    pub repo_id: i64,
    pub path: String,
    pub branch: String,
    pub tmux_window: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: Option<i64>,
    pub name: String,
    pub repo_id: Option<i64>,
    pub base_branch: Option<String>,
    #[allow(dead_code)]
    pub created_at: Option<String>,
    pub last_selected_at: Option<String>,
    pub windows: Option<i64>,
    pub attached: bool,
    pub live: bool,
    pub managed: bool,
}

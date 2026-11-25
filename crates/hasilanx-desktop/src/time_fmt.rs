use hasilanx_protocol::now_ms;

pub fn relative_time(ts_ms: i64) -> String {
    let diff_s = ((now_ms() - ts_ms) / 1000).max(0);
    if diff_s < 45 {
        "now".to_string()
    } else if diff_s < 3600 {
        format!("{}m", diff_s / 60)
    } else if diff_s < 86_400 {
        format!("{}h", diff_s / 3600)
    } else {
        format!("{}d", diff_s / 86_400)
    }
}

pub fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

pub fn agent_display_name(agent_id: &str) -> &'static str {
    match agent_id {
        "codex" => "Codex",
        "agy" => "Antigravity",
        "opencode" => "OpenCode",
        _ => "Agent",
    }
}

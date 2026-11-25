use hasilanx_adapters::{AdapterRegistry, StartSessionParams};
use hasilanx_protocol::ExecutionMode;
use std::path::PathBuf;

#[tokio::test]
async fn live_adapter_session_handshakes() {
    let reg = AdapterRegistry::discover(&[]);
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    for agent_id in ["codex", "agy", "opencode"] {
        let Some(desc) = reg.get(agent_id) else {
            panic!("missing builtin agent `{agent_id}`");
        };
        if !desc.available {
            eprintln!("skipping `{agent_id}` (not installed)");
            continue;
        }

        let res = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            reg.start_session(
                agent_id,
                StartSessionParams {
                    cwd: cwd.clone(),
                    resume_session_id: None,
                    execution_mode: ExecutionMode::Ask,
                    model: None,
                    thinking: None,
                },
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("timeout starting `{agent_id}` session"))
        .unwrap_or_else(|err| panic!("failed starting `{agent_id}` session: {err:#}"));

        let (session, _rx) = res;
        let sid = session
            .vendor_session_id()
            .unwrap_or_else(|| panic!("`{agent_id}` returned no vendor_session_id"));
        assert!(!sid.is_empty(), "`{agent_id}` returned empty session id");
        eprintln!("verified `{agent_id}` session_id={sid}");

        let updated = reg
            .refresh_agent_models(agent_id)
            .await
            .unwrap_or_else(|err| panic!("failed live model discovery for `{agent_id}`: {err:#}"));
        assert!(
            !updated.models.is_empty(),
            "`{agent_id}` returned 0 live models from app-server/ACP"
        );
        assert!(
            !updated.thinking_options.is_empty(),
            "`{agent_id}` returned 0 live thinking options from app-server/ACP"
        );
        eprintln!(
            "verified `{agent_id}` live models: count={} default={:?} first={} | thinking: count={} default={:?} opts={:?}",
            updated.models.len(),
            updated.default_model,
            updated.models[0].id,
            updated.thinking_options.len(),
            updated.default_thinking,
            updated
                .thinking_options
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
        );
    }
}

use animus_plugin_protocol::{PluginInfo, PLUGIN_KIND_SUBJECT_BACKEND};
use animus_plugin_runtime::subject_backend_main_with_capabilities;
use animus_subject_requirements::backend::RequirementsBackend;
use animus_subject_requirements::config::RequirementsConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    emit_manifest_if_requested();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let config = RequirementsConfig::from_env()?;
    let backend = RequirementsBackend::new(config).await?;

    let info = PluginInfo {
        name: env!("CARGO_PKG_NAME").into(),
        version: env!("CARGO_PKG_VERSION").into(),
        plugin_kind: PLUGIN_KIND_SUBJECT_BACKEND.into(),
        description: Some(env!("CARGO_PKG_DESCRIPTION").into()),
    };

    subject_backend_main_with_capabilities(
        info,
        backend,
        vec!["subject_kind:requirement".to_string()],
    )
    .await
}

fn emit_manifest_if_requested() {
    if !std::env::args()
        .skip(1)
        .any(|arg| arg == "--manifest" || arg == "-m")
    {
        return;
    }

    let manifest = serde_json::json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "plugin_kind": "subject_backend",
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "protocol_version": animus_plugin_protocol::PROTOCOL_VERSION,
        "capabilities": [
            "requirement/list",
            "requirement/get",
            "requirement/update",
            "requirement/delete",
            "requirement/schema",
            "subject/list",
            "subject/get",
            "subject/update",
            "subject/delete",
            "subject/schema",
            "health/check",
            "subject_kind:requirement"
        ],
        "env_required": [
            {
                "name": "ANIMUS_REQUIREMENTS_ROOT",
                "description": "Root directory for requirement files.",
                "required": false
            },
            {
                "name": "ANIMUS_REQUIREMENTS_ID_PREFIX",
                "description": "ID prefix for requirement records.",
                "required": false
            },
            {
                "name": "ANIMUS_REQUIREMENTS_INDEX_TTL_SECS",
                "description": "Requirement index cache TTL in seconds.",
                "required": false
            },
            {
                "name": "ANIMUS_PROJECT_ROOT",
                "description": "Active project root used to resolve default storage.",
                "required": false
            },
            {
                "name": "ANIMUS_REQUIREMENTS_LEGACY_JSON",
                "description": "Legacy core-state.json path for read/migrate compatibility.",
                "required": false
            },
            {
                "name": "ANIMUS_SCOPED_ROOT",
                "description": "Scoped runtime root used to locate legacy core-state.json.",
                "required": false
            },
            {
                "name": "ANIMUS_REQUIREMENTS_MIGRATE_LEGACY",
                "description": "Run one-shot legacy JSON to Markdown migration on startup.",
                "required": false
            }
        ]
    });
    println!(
        "{}",
        serde_json::to_string(&manifest).expect("serialize manifest")
    );
    std::process::exit(0);
}

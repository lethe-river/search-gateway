use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::ServiceExt;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn mcp_lists_five_tools_and_handles_unknown_search_id() -> anyhow::Result<()> {
    let directory = TempDir::new()?;
    let config_path = directory.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[searxng]
enabled = true
base_url = "http://127.0.0.1:1"

[tavily]
enabled = false

[firecrawl]
enabled = false
"#,
    )?;

    let binary = env!("CARGO_BIN_EXE_search-gateway");
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(binary).configure(|command| {
            command.arg("--config").arg(&config_path);
        }),
    )?;

    let client = ().serve(transport).await?;
    let tools = client.list_all_tools().await?;
    let mut names: Vec<String> = tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "doctor",
            "get_sources",
            "web_fetch",
            "web_map",
            "web_search"
        ]
    );
    for tool in &tools {
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
        assert!(annotations.title.is_some(), "{}", tool.name);
    }

    let arguments = json!({"search_id": "does-not-exist"})
        .as_object()
        .expect("object")
        .clone();
    let result = client
        .call_tool(CallToolRequestParams::new("get_sources").with_arguments(arguments))
        .await?;
    let structured = result
        .structured_content
        .expect("structured content");
    assert_eq!(structured["error"]["code"], "unknown_search_id");
    assert_eq!(result.is_error, Some(true));

    let arguments = json!({"url": "http://127.0.0.1:1/"})
        .as_object()
        .expect("object")
        .clone();
    let fetch = client
        .call_tool(CallToolRequestParams::new("web_fetch").with_arguments(arguments))
        .await?;
    let structured = fetch
        .structured_content
        .expect("structured content");
    assert_eq!(structured["error"]["code"], "blocked");
    assert_eq!(fetch.is_error, Some(true));

    let arguments = json!({"url": "https://example.com/", "fetch_id": "does-not-exist"})
        .as_object()
        .expect("object")
        .clone();
    let continuation = client
        .call_tool(CallToolRequestParams::new("web_fetch").with_arguments(arguments))
        .await?;
    let structured = continuation
        .structured_content
        .expect("structured content");
    assert_eq!(structured["error"]["code"], "unknown_fetch_id");
    assert_eq!(continuation.is_error, Some(true));

    let arguments = json!({"query": "no providers configured"})
        .as_object()
        .expect("object")
        .clone();
    let search = client
        .call_tool(CallToolRequestParams::new("web_search").with_arguments(arguments))
        .await?;
    let structured = search
        .structured_content
        .expect("structured content");
    assert_eq!(structured["sources"].as_array().map(Vec::len), Some(0));
    assert!(!structured["provider_errors"]
        .as_object()
        .expect("provider errors")
        .is_empty());
    assert_eq!(search.is_error, Some(true));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn clean_zero_result_search_is_not_an_mcp_error() -> anyhow::Result<()> {
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"results": []})),
        )
        .mount(&upstream)
        .await;

    let directory = TempDir::new()?;
    let config_path = directory.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[tavily]
enabled = true
base_url = "{}"
keys = ["test-key"]

[firecrawl]
enabled = false

[searxng]
enabled = false

[cache]
persist = false
"#,
            upstream.uri()
        ),
    )?;

    let binary = env!("CARGO_BIN_EXE_search-gateway");
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(binary).configure(|command| {
            command.arg("--config").arg(&config_path);
        }),
    )?;
    let client = ().serve(transport).await?;

    let arguments = json!({"query": "no matching documents"})
        .as_object()
        .expect("object")
        .clone();
    let result = client
        .call_tool(CallToolRequestParams::new("web_search").with_arguments(arguments))
        .await?;
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["sources"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        structured["provider_errors"]["chain"]["code"],
        "zero_results"
    );
    assert_eq!(structured["meta"]["cached"], false);
    assert_eq!(
        result.is_error,
        Some(false),
        "a clean zero result is a successful search"
    );

    client.cancel().await?;
    Ok(())
}

use ai_tester::assertions::{compute_weighted_score, evaluate_assertions};
use ai_tester::config::load_project_config;
use ai_tester::runtime::{parse_claude_jsonl, parse_codex_jsonl};
use ai_tester::scenario::{AssertionSpec, Scenario};
use ai_tester::skill::allowed_tools::tokenize_allowed_tools;
use ai_tester::skill::parse_skill_md;
use ai_tester::trace::{ToolCallRecord, TraceRecord, Turn};
use ai_tester::util::regex::compile_pattern;
use tempfile::TempDir;

#[test]
fn scenario_parses_minimal_skill_with_defaults() {
    let yaml = "scenario: basic\nskill: aif-commit\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");

    assert_eq!(scenario.scenario, "basic");
    assert_eq!(scenario.skill.as_deref(), Some("aif-commit"));
    assert_eq!(scenario.runner.runtime, "claude");
    assert_eq!(scenario.runner.permission_mode, "bypassPermissions");
    assert!(scenario.assertions.is_empty());
    assert!(scenario.user_responses.is_empty());
    assert!(!scenario.fixtures.git_init);
}

#[test]
fn scenario_accepts_token_budget_alias_and_rejects_prompt_conflicts() {
    let yaml = "scenario: budget\nskill: aif-commit\ntoken-budget: 123\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("hyphenated token budget parses");
    assert_eq!(scenario.token_budget, Some(123.0));

    let bad = "scenario: conflict\nskill: aif-commit\nsystem_prompt: hi\n";
    let err = Scenario::from_yaml_str(bad).expect_err("multiple prompt sources rejected");
    assert!(err.to_string().contains("exactly one"));
}

#[test]
fn allowed_tools_tokenizer_preserves_scopes() {
    let parsed = tokenize_allowed_tools(Some("Bash(git *), Read, mcp__server__tool(scope, two)"));

    assert_eq!(
        parsed.raw,
        vec!["Bash(git *)", "Read", "mcp__server__tool(scope, two)"]
    );
    assert_eq!(parsed.parsed[0].name, "Bash");
    assert_eq!(parsed.parsed[0].scopes, vec!["git *"]);
    assert_eq!(parsed.parsed[2].scopes, vec!["scope", "two"]);
}

#[test]
fn skill_frontmatter_parses_body_hash_and_token_budget() {
    let tmp = TempDir::new().expect("temp dir");
    let skill = tmp.path().join("SKILL.md");
    std::fs::write(
        &skill,
        "---\nname: demo\ndescription: Demo skill\nallowed-tools: Bash(git *), Read\ntoken-budget: 5000\n---\n\nBody text\n",
    )
    .expect("skill written");

    let parsed = parse_skill_md(&skill).expect("skill parses");
    assert_eq!(parsed.frontmatter.name, "demo");
    assert_eq!(parsed.frontmatter.description, "Demo skill");
    assert_eq!(parsed.frontmatter.token_budget, Some(5000.0));
    assert_eq!(parsed.allowed_tools.raw, vec!["Bash(git *)", "Read"]);
    assert_eq!(parsed.body.trim(), "Body text");
    assert_eq!(parsed.body_hash.len(), 64);
}

#[test]
fn project_config_walks_up_and_resolves_skills_dir() {
    let tmp = TempDir::new().expect("temp dir");
    let nested = tmp.path().join("a/b");
    std::fs::create_dir_all(&nested).expect("nested created");
    std::fs::write(
        tmp.path().join(".ai-tester.yaml"),
        "skills_dir: ./custom-skills\ndefaults:\n  model: custom\n  permission_mode: plan\n",
    )
    .expect("config written");

    let config = load_project_config(&nested).expect("config loads");
    assert_eq!(config.root_dir, tmp.path());
    assert_eq!(config.skills_dir, tmp.path().join("custom-skills"));
    assert_eq!(config.defaults.model.as_deref(), Some("custom"));
    assert_eq!(config.defaults.permission_mode.as_deref(), Some("plan"));
}

#[test]
fn regex_supports_leading_inline_flags() {
    let re = compile_pattern("(?is)hello.world").expect("regex compiles");
    assert!(re.is_match("HELLO\nworld"));
}

#[test]
fn assertions_evaluate_tool_calls_output_turns_and_token_budget() {
    let trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool("1", "Bash", serde_json::json!({"command": "git status"})),
            Turn::assistant_with_tool(
                "2",
                "Bash",
                serde_json::json!({"command": "git commit -m feat"}),
            ),
        ],
        "Commit created: feat(auth): add login".to_string(),
        5,
        Some(100.0),
    );

    let assertions = vec![
        AssertionSpec::tool_called(
            "calls-status",
            "Bash",
            serde_json::json!({"command": "^git status"}),
        ),
        AssertionSpec::no_tool_called("no-write", "Write"),
        AssertionSpec::output_contains("mentions-feat", "\\bfeat\\b"),
        AssertionSpec::turn_count_at_most("efficient", 6),
    ];

    let results = evaluate_assertions(&assertions, &trace);
    assert!(results.iter().all(|r| r.pass), "{results:#?}");
    assert_eq!(compute_weighted_score(&results), 1.0);
}

#[test]
fn assertions_report_failures_for_unwanted_tool_and_unanswered_questions() {
    let mut trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool(
                "1",
                "Write",
                serde_json::json!({"file_path": "../escape.txt"}),
            ),
            Turn::assistant_with_tool(
                "2",
                "AskUserQuestion",
                serde_json::json!({"question": "Proceed?"}),
            ),
        ],
        "done".to_string(),
        10,
        None,
    );
    trace.tool_call_summary.unanswered_questions = 1;

    let assertions = vec![
        AssertionSpec::no_tool_called("no-write", "Write"),
        AssertionSpec::turn_count_at_most("efficient", 6),
    ];

    let results = evaluate_assertions(&assertions, &trace);
    let failed: Vec<_> = results
        .iter()
        .filter(|r| !r.pass)
        .map(|r| r.id.as_str())
        .collect();
    assert!(failed.contains(&"no-write"));
    assert!(failed.contains(&"efficient"));
    assert!(failed.contains(&"no_unanswered_questions"));
}

#[test]
fn trace_serializes_with_v2_schema_version() {
    let trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "1",
            "Bash",
            serde_json::json!({"command": "git status"}),
        )],
        "ok".to_string(),
        1,
        None,
    );
    let json = serde_json::to_value(&trace).expect("trace serializes");

    assert_eq!(json["schemaVersion"], "2.0.0");
    assert_eq!(json["toolCallSummary"]["byTool"]["Bash"], 1);
}

#[test]
fn synthetic_tool_call_records_result_defaults() {
    let call = ToolCallRecord::new("abc", "Bash", serde_json::json!({"command": "git status"}));

    assert_eq!(call.id, "abc");
    assert_eq!(call.name, "Bash");
    assert!(!call.result_is_error);
    assert!(call.result_content.is_none());
}

#[test]
fn codex_jsonl_parser_normalizes_shell_and_message_events() {
    let jsonl = r#"{"type":"thread.started","thread_id":"thread-1"}
{"type":"turn.started"}
{"type":"item.started","item":{"type":"command_execution","id":"cmd-1","command":"git status"}}
{"type":"item.completed","item":{"type":"command_execution","id":"cmd-1","status":"completed","aggregated_output":"clean"}}
{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":2,"cached_input_tokens":3}}
"#;

    let parsed = parse_codex_jsonl(jsonl, 40, false).expect("codex jsonl parses");
    assert_eq!(parsed.session_id.as_deref(), Some("thread-1"));
    assert_eq!(parsed.final_output, "done");
    assert_eq!(parsed.turns[0].tool_calls[0].name, "Bash");
    assert_eq!(
        parsed.turns[0].tool_calls[0].result_content.as_deref(),
        Some("clean")
    );
    assert_eq!(parsed.cost.input_tokens, 10);
    assert_eq!(parsed.cost.cache_read_tokens, 3);
}

#[test]
fn claude_jsonl_parser_normalizes_tool_results_and_usage() {
    let jsonl = r#"{"type":"system","subtype":"init","session_id":"session-1"}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"git status"}},{"type":"text","text":"hello"}],"usage":{"input_tokens":1,"output_tokens":2}}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"clean","is_error":false}]}}
{"type":"result","subtype":"success","result":"done","usage":{"input_tokens":3,"output_tokens":4,"cache_creation_input_tokens":5,"cache_read_input_tokens":6},"total_cost_usd":0.01}
"#;

    let parsed = parse_claude_jsonl(jsonl, 12, true).expect("claude jsonl parses");
    assert_eq!(parsed.session_id.as_deref(), Some("session-1"));
    assert_eq!(parsed.final_output, "done");
    assert_eq!(parsed.turns[0].text_deltas, vec!["hello"]);
    assert_eq!(
        parsed.turns[0].tool_calls[0].result_content.as_deref(),
        Some("clean")
    );
    assert_eq!(parsed.cost.input_tokens, 3);
    assert_eq!(parsed.cost.cache_creation_tokens, 5);
    assert_eq!(parsed.cost.usd_estimate, 0.01);
    assert!(parsed.max_turns_user_set);
}

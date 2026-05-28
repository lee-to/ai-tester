use ai_tester::assertions::{compute_weighted_score, evaluate_assertions};
use ai_tester::config::load_project_config;
use ai_tester::runtime::{
    parse_claude_jsonl, parse_claude_jsonl_with_user_responses, parse_codex_jsonl,
};
use ai_tester::scenario::{load_scenario_file, AssertionSpec, Scenario, UserResponse};
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
fn scenario_loader_tracks_explicit_runner_fields() {
    let tmp = TempDir::new().expect("temp dir");
    let scenario_path = tmp.path().join("scenario.yaml");
    std::fs::write(
        &scenario_path,
        "scenario: defaults\nsystem_prompt: Body\nrunner:\n  runtime: codex\n",
    )
    .expect("scenario written");

    let loaded = load_scenario_file(&scenario_path).expect("scenario loads");
    assert!(loaded.source_meta.runner_runtime_set);
    assert!(!loaded.source_meta.runner_model_set);
    assert!(!loaded.source_meta.runner_permission_mode_set);

    std::fs::write(
        &scenario_path,
        "scenario: explicit\nsystem_prompt: Body\nrunner:\n  model: explicit-model\n  permission_mode: plan\n",
    )
    .expect("scenario written");
    let loaded = load_scenario_file(&scenario_path).expect("scenario loads");
    assert!(!loaded.source_meta.runner_runtime_set);
    assert!(loaded.source_meta.runner_model_set);
    assert!(loaded.source_meta.runner_permission_mode_set);
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
        AssertionSpec::no_output_contains("no-regression", "panic|error"),
        AssertionSpec::turn_count_at_most("efficient", 6),
    ];

    let results = evaluate_assertions(&assertions, &trace);
    assert!(results.iter().all(|r| r.pass), "{results:#?}");
    assert_eq!(compute_weighted_score(&results), 1.0);
}

#[test]
fn tool_called_accepts_tool_pattern() {
    let yaml = "scenario: pattern\nsystem_prompt: Body\nassertions:\n  - id: codegraph\n    type: tool_called\n    tool_pattern: '^mcp__.*__codegraph_context$'\n";
    let scenario = Scenario::from_yaml_str(yaml).expect("scenario parses");
    let trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "1",
            "mcp__ai_workspace__codegraph_context",
            serde_json::json!({"task": "find bug"}),
        )],
        "done".to_string(),
        1,
        None,
    );

    let results = evaluate_assertions(&scenario.assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "codegraph")
        .expect("assertion result");
    assert!(result.pass, "{results:#?}");
}

#[test]
fn no_output_contains_fails_when_final_output_matches() {
    let trace = TraceRecord::synthetic(Vec::new(), "WARN [+check] failed".to_string(), 1, None);
    let assertions = vec![AssertionSpec::no_output_contains(
        "no-check-warning",
        "WARN \\[\\+check\\]",
    )];

    let results = evaluate_assertions(&assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "no-check-warning")
        .expect("assertion result");
    assert!(!result.pass, "{results:#?}");
    assert_eq!(result.kind, "no_output_contains");
}

#[test]
fn file_read_matches_claude_read_and_codex_bash_readers() {
    let assertions = vec![AssertionSpec::file_read(
        "reads-runtime",
        "src/runtime/mod\\.rs",
    )];

    let claude_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "read-1",
            "Read",
            serde_json::json!({"file_path": "src/runtime/mod.rs"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let claude_results = evaluate_assertions(&assertions, &claude_trace);
    assert!(
        claude_results
            .iter()
            .any(|result| result.id == "reads-runtime" && result.pass),
        "{claude_results:#?}"
    );

    let codex_trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "bash-1",
            "Bash",
            serde_json::json!({"command": "sed -n '1,220p' src/runtime/mod.rs"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let codex_results = evaluate_assertions(&assertions, &codex_trace);
    assert!(
        codex_results
            .iter()
            .any(|result| result.id == "reads-runtime" && result.pass),
        "{codex_results:#?}"
    );
}

#[test]
fn file_read_does_not_match_non_reader_bash_mentions() {
    let trace = TraceRecord::synthetic(
        vec![Turn::assistant_with_tool(
            "bash-1",
            "Bash",
            serde_json::json!({"command": "cargo test src/runtime/mod.rs"}),
        )],
        "done".to_string(),
        1,
        None,
    );
    let assertions = vec![AssertionSpec::file_read(
        "reads-runtime",
        "src/runtime/mod\\.rs",
    )];

    let results = evaluate_assertions(&assertions, &trace);
    let result = results
        .iter()
        .find(|result| result.id == "reads-runtime")
        .expect("assertion result");
    assert!(!result.pass, "{results:#?}");
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
fn no_path_escape_inspects_tool_path_inputs_against_sandbox() {
    let tmp = TempDir::new().expect("temp dir");
    let mut trace = TraceRecord::synthetic(
        vec![
            Turn::assistant_with_tool("1", "Write", serde_json::json!({"file_path": "src/lib.rs"})),
            Turn::assistant_with_tool(
                "2",
                "Read",
                serde_json::json!({"file_path": "../escape.txt"}),
            ),
        ],
        "done".to_string(),
        2,
        None,
    );
    trace.runner.sandbox_path = Some(tmp.path().to_string_lossy().to_string());

    let assertions = vec![AssertionSpec::NoPathEscape {
        id: "paths-stay-inside".to_string(),
        weight: 1.0,
        tools: None,
        allow_outside: None,
    }];
    let results = evaluate_assertions(&assertions, &trace);

    let path_result = results
        .iter()
        .find(|result| result.id == "paths-stay-inside")
        .expect("path assertion exists");
    assert!(!path_result.pass);
    assert!(path_result.detail.contains("../escape.txt"));
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

#[test]
fn claude_jsonl_parser_does_not_mark_subprocess_questions_answered() {
    let jsonl = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"q-1","name":"AskUserQuestion","input":{"question":"Proceed with commit?"}}]}}
{"type":"result","subtype":"success","result":"done"}
"#;

    let unanswered =
        parse_claude_jsonl_with_user_responses(jsonl, 12, false, &[]).expect("jsonl parses");
    assert_eq!(unanswered.unanswered_questions, 1);
    assert!(unanswered.turns[0].tool_calls[0].answered.is_none());

    let unsupported = parse_claude_jsonl_with_user_responses(
        jsonl,
        12,
        false,
        &[UserResponse {
            match_question: "Proceed".to_string(),
            choose: "Yes".to_string(),
        }],
    )
    .expect("jsonl parses");
    assert_eq!(unsupported.unanswered_questions, 1);
    assert!(unsupported.turns[0].tool_calls[0].answered.is_none());
    assert!(unsupported
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("cannot deliver user_responses")));
}

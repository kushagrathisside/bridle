# Checklist: every stage, built and plugged in

Every row says what was built, where it lives, which tests prove it, and where it is plugged into an entry point (a CLI
command, a tool list, a hook, a provider, a validation rule). `scripts/check_checklist.py` checks each built or partial row:
the files exist, the tests exist, and the plug symbol appears in its file. An open row names what is missing.

Status: `built` means implemented, tested and plugged in. `partial` means built, with a part not verified live or not
finished. `open` means not built.

| Id | Stage | Item | Status | Built in | Tests | Plugged into | Requirements |
|---|---|---|---|---|---|---|---|
| M0-1 | M0 | Architecture and audit | built | docs/concepts/architecture.md | | docs/concepts/architecture.md::Request flow | R-33.2 |
| M0-2 | M0 | Thin host adapter decision | built | docs/decisions/0001-rust-core-thin-host-adapter.md | | docs/decisions/0001-rust-core-thin-host-adapter.md::Decision | D-1 |
| M1-1 | M1 | Config schema and validation | built | crates/harness-core/src/config/schema.rs, crates/harness-core/src/config/validate.rs | v4_remote_endpoint_needs_cloud_opt_in | crates/harness-cli/src/app.rs::require_valid_config | R-6, R-27.4 |
| M1-2 | M1 | Redaction of secret values | built | crates/harness-core/src/security/redact.rs | pem_private_key_block_is_redacted_whole, authorization_header_keeps_name_and_redacts_rest_of_line | crates/harness-cli/src/commands/hook.rs::Redactor::new | R-27.2, R-27.3 |
| M1-3 | M1 | Command-risk classifier | built | crates/harness-core/src/security/command_risk.rs | read_only_commands, never_panics_on_generated_garbage | crates/harness-core/src/security/tool_policy.rs::assess | R-27.1 |
| M1-4 | M1 | Path guard | built | crates/harness-core/src/security/path_guard.rs | traversal_out_of_the_root_is_outside_the_workspace, symlink_to_a_directory_outside_the_root_is_a_symlink_escape | crates/harness-core/src/security/tool_policy.rs::guard.check | R-27.1 |
| M1-5 | M1 | Hook answers and fails closed | built | crates/harness-cli/src/host/opencode_v1.rs, crates/harness-cli/src/commands/hook.rs | every_hook_fixture_row_matches_the_policy | crates/harness-cli/src/commands/hook.rs::failure_answer | R-27.4 |
| M1-6 | M1 | Launcher argv for the server | built | crates/harness-runtime/src/launcher.rs | | crates/harness-cli/src/commands/models.rs::launcher::spawn_server | R-6.1 |
| M1-7 | M1 | OpenCode shim (TypeScript) | built | opencode-plugin/local-agent.ts | | crates/harness-cli/src/commands/init.rs::SHIM_TEMPLATE | R-33.2 |
| M1-8 | M1 | Doctor, health and models commands | built | crates/harness-cli/src/commands/doctor.rs, crates/harness-cli/src/commands/health.rs, crates/harness-cli/src/commands/models.rs | unreachable_endpoint_is_a_failed_health_check | crates/harness-cli/src/main.rs::Command::Doctor | R-42.2 |
| M1-9 | M1 | Event log with redaction | built | crates/harness-runtime/src/event_log.rs | appended_events_never_contain_secret_values | crates/harness-cli/src/commands/hook.rs::event_log().append | R-27.3 |
| M1-10 | M1 | Real model run through OpenCode | built | docs/status/progress.md | | crates/harness-cli/src/main.rs::Command::Doctor | R-6.1 |
| M2-1 | M2 | MCP server on stdio | built | crates/harness-runtime/src/mcp/protocol.rs | tools_list_reports_default_tools_in_order | crates/harness-cli/src/commands/mcp.rs::Server::new | R-7.x |
| M2-2 | M2 | Git tools | built | crates/harness-runtime/src/mcp/tools/mod.rs | | crates/harness-runtime/src/mcp/tools/mod.rs::Box::new(GitStatusTool) | R-14.1 |
| M2-3 | M2 | Verification tools and failure parsing | built | crates/harness-runtime/src/mcp/tools/verify.rs, crates/harness-runtime/src/mcp/tools/failures.rs, crates/harness-core/src/failures.rs | every_pattern_compiles | crates/harness-runtime/src/mcp/tools/mod.rs::Box::new(verify::RUN_TESTS) | R-14.2 |
| M2-4 | M2 | Code navigation with tree-sitter | built | crates/harness-runtime/src/mcp/tools/code.rs | | crates/harness-runtime/src/mcp/tools/mod.rs::Box::new(code::RepoMapTool) | R-14.2 |
| M2-5 | M2 | Dependency search | built | crates/harness-runtime/src/mcp/tools/deps.rs | | crates/harness-runtime/src/mcp/tools/mod.rs::Box::new(deps::DependencySearchTool) | R-14.3 |
| M2-6 | M2 | Artifact and experiment tools | built | crates/harness-runtime/src/artifacts/mod.rs | | crates/harness-runtime/src/mcp/tools/mod.rs::Box::new(artifacts::InspectArtifactTool) | R-14.4 |
| M2-7 | M2 | Declarative adapter tools | built | crates/harness-runtime/src/mcp/tools/adapters.rs | | crates/harness-cli/src/commands/mcp.rs::adapter_tools | R-14.4 |
| M3-1 | M3 | Per-task state store | built | crates/harness-runtime/src/state/mod.rs, crates/harness-runtime/src/state/task.rs | | crates/harness-cli/src/commands/task.rs::start_task | R-16.1 |
| M3-2 | M3 | Task journal and fold | built | crates/harness-core/src/journal.rs | a_failed_test_run_goes_through_fail_to_diagnose | crates/harness-cli/src/workflow.rs::CallStarted | R-16.3 |
| M3-3 | M3 | Loop rules | built | crates/harness-core/src/loops.rs | the_fourth_identical_command_is_refused_through_the_real_hook | crates/harness-cli/src/workflow.rs::detect | R-20.1 |
| M3-4 | M3 | Rules decision engine and refusals | built | crates/harness-core/src/decision.rs | an_unreachable_model_falls_back_to_the_rules_and_the_refusal_still_happens | crates/harness-cli/src/workflow.rs::RulesDecisionEngine | R-9.4, R-17.1 |
| M3-5 | M3 | Task commands: start, status, reset, trace, explain | built | crates/harness-cli/src/commands/task.rs | | crates/harness-cli/src/main.rs::Command::Task | R-54.2, R-54.3 |
| M3-6 | M3 | Worktree tasks | built | crates/harness-cli/src/commands/task.rs | a_worktree_task_runs_in_its_own_folder_and_leaves_the_primary_tree_alone | crates/harness-cli/src/commands/task.rs::start_in_worktree | R-26.1 |
| M3-7 | M3 | task_status tool, off by default | built | crates/harness-runtime/src/mcp/tools/task.rs | | crates/harness-cli/src/commands/mcp.rs::TaskStatusTool | R-57.3 |
| M4-1 | M4 | Micro model decides stalled loops | built | crates/harness-runtime/src/decision.rs, crates/harness-core/src/lfm.rs | | crates/harness-cli/src/workflow.rs::LfmEngine | R-4.3, R-20.2, R-30.2 |
| M4-2 | M4 | Strict check of the micro answer | built | crates/harness-core/src/lfm.rs | | crates/harness-runtime/src/decision.rs::lfm::parse_choice | R-30.4 |
| M4-3 | M4 | Decision evaluation command | built | crates/harness-cli/src/commands/eval.rs | | crates/harness-cli/src/main.rs::Command::EvalDecisions | R-39.5 |
| M5-1 | M5 | One GPU model at a time | built | crates/harness-cli/src/commands/models.rs | | crates/harness-cli/src/commands/models.rs::check_one_gpu_model | R-45.1 |
| M5-2 | M5 | Deep model on demand | built | crates/harness-cli/src/commands/models.rs | | crates/harness-cli/src/main.rs::Command::EvalDecisions | R-45.3, R-3.5 |
| M5-3 | M5 | Escalation to the deep slot | partial | crates/harness-cli/src/workflow.rs | an_escalation_is_off_unless_routing_enables_it, a_due_escalation_goes_to_the_deep_slot_and_a_failure_keeps_the_case, the_escalations_a_task_may_use_are_capped, an_escalation_needs_a_deep_slot, only_an_escalate_decision_is_escalated, an_escalation_takes_the_deep_answer_and_keeps_its_reason_code, a_failed_escalation_keeps_the_escalated_decision_and_says_why, an_escalate_decision_is_due_when_its_confidence_reaches_the_threshold, v39_deep_escalation_needs_a_deep_slot, v40_local_lfm_without_micro_routing_is_a_warning | crates/harness-cli/src/workflow.rs::escalate_if_due | R-3.6, R-18.1 |
| M6-1 | M6 | Context policy | built | crates/harness-core/src/context.rs | the_budget_is_taken_in_tokens, recent_results_are_kept_whole_while_the_context_fits, a_recent_result_is_compacted_only_when_the_context_would_not_fit | crates/harness-cli/src/host/opencode_v1.rs::messages_transform | R-24.1, R-24.2 |
| M6-2 | M6 | Per-request tool-result transform | built | crates/harness-cli/src/host/opencode_v1.rs | older_reads_are_dropped_over_the_hard_limit_and_the_recent_ones_are_kept, an_old_verification_keeps_only_its_failure_lines | crates/harness-cli/src/host/opencode_v1.rs::HookEvent::MessagesTransform | R-23.2, R-33.1 |
| M6-3 | M6 | Task state in the system prompt | built | crates/harness-cli/src/workflow.rs | the_state_block_names_the_phase_and_cleans_model_text | crates/harness-cli/src/commands/hook.rs::HookEvent::SystemTransform | R-23.1 |
| M6-4 | M6 | Shim forwards both experimental hooks | built | opencode-plugin/local-agent.ts | | opencode-plugin/local-agent.ts::experimental.chat.messages.transform | R-33.1 |
| M7-1 | M7 | Jev answer contract | built | crates/harness-core/src/jev.rs | a_checked_answer_keeps_its_probability_and_reason, answers_outside_the_contract_are_rejected | crates/harness-runtime/src/jev.rs::parse_decision | R-9.5 |
| M7-2 | M7 | Jev decision engine | partial | crates/harness-runtime/src/jev.rs | a_checked_answer_is_used_with_its_probability_and_the_key_goes_in_a_bearer_header, a_failed_call_falls_back_to_the_rules_and_never_shows_the_key | crates/harness-cli/src/workflow.rs::JevEngine::from_config | R-9.1, R-36.3 |
| M7-3 | M7 | Jev endpoint policy V30 to V35 | built | crates/harness-core/src/config/validate.rs | v30_to_v35_guard_the_jev_endpoint | crates/harness-core/src/config/validate.rs::check_jev_endpoint | R-27.4 |
| M7-4 | M7 | TLS to a public host | built | crates/harness-runtime/src/http.rs | tls_handshake_reaches_a_public_host | crates/harness-runtime/src/http.rs::remote_agent | R-27.4 |
| M7-5 | M7 | Jev live decisions | open | | | | R-57.7 |
| M8-1 | M8 | LiteLLM gateway as a model provider | built | crates/harness-core/src/config/schema.rs, crates/harness-runtime/src/providers/openai_compat.rs | a_litellm_slot_sends_its_route_name_and_the_gateway_key_from_its_variable | crates/harness-runtime/src/providers/mod.rs::ProviderKind::Litellm | R-7.2, R-8.3 |
| M8-2 | M8 | Gateway key policy V36 | built | crates/harness-core/src/config/validate.rs | v36_a_remote_litellm_gateway_needs_a_key_name | crates/harness-core/src/config/validate.rs::V36 | R-27.5 |
| M8-3 | M8 | Hybrid profile and key reference in OpenCode config | built | configs/hybrid.toml, crates/harness-cli/src/host/opencode_v1.rs | | crates/harness-cli/src/host/opencode_v1.rs::api_key_reference | R-8.3 |
| M8-4 | M8 | Live gateway check | open | | | | R-8.3 |
| M9-1 | M9 | Command, path and secret checks, with the M9 gaps closed | built | crates/harness-core/src/security/tool_policy.rs | escaped_words_are_judged_as_the_shell_reads_them, short_name_aliases_of_a_file_are_denied_as_possible_secrets, commands_that_dump_the_environment_are_denied | crates/harness-core/src/security/tool_policy.rs::dumps_environment | R-27.1, R-27.2 |
| M9-2 | M9 | Adversarial suite of 72 cases | built | crates/harness-cli/tests/adversarial.rs, evals/security/cases.json | every_hostile_case_gets_its_expected_verdict | crates/harness-cli/tests/adversarial.rs::CASES | R-57.9 |
| M9-3 | M9 | Egress audit command | built | crates/harness-cli/src/commands/egress.rs | the_shipped_local_profile_reaches_only_loopback, a_remote_jev_endpoint_is_listed_as_remote_with_its_condition | crates/harness-cli/src/main.rs::Command::Egress | R-27.4 |
| M10-1 | M10 | Benchmark runner and report | built | crates/harness-cli/src/commands/bench.rs | the_report_carries_its_caveat_and_one_row_per_task, the_log_counts_calls_refusals_and_context_bytes | crates/harness-cli/src/main.rs::Command::Bench | R-39.1, R-57.10 |
| M10-2 | M10 | Task corpus with verify scripts | built | evals/tasks/debug-range-sum/verify.py, evals/tasks/implement-fizzbuzz/verify.py | | evals/tasks/debug-range-sum/verify.py::range_sum | R-39.1, R-40.2 |
| M10-3 | M10 | Live benchmark results | partial | evals/bench/results/2026-10-27/report.md, docs/status/milestones/m10-evaluation.md | the_blocked_registry_is_loopback_so_nothing_leaves_the_machine | crates/harness-cli/src/commands/bench.rs::BLOCKED_REGISTRY | R-39.4, R-39.5 |
| M11-1 | M11 | Apache-2.0 licence and NOTICE | built | LICENSE, NOTICE | | Cargo.toml::license = "Apache-2.0" | R-52.1 |
| M11-2 | M11 | Third-party licence list and generator | built | THIRD_PARTY_LICENSES.md, scripts/third_party_licenses.py | | scripts/third_party_licenses.py::cargo | R-52.1, AC-R3 |
| M11-3 | M11 | Changelog, security policy and contributing guide | built | CHANGELOG.md, SECURITY.md, CONTRIBUTING.md | | SECURITY.md::private vulnerability | R-57.11 |
| M11-4 | M11 | Continuous integration workflow | partial | .github/workflows/ci.yml | | .github/workflows/ci.yml::cargo clippy | AC-R4 |
| M11-5 | M11 | Crates inherit the licence | built | crates/harness-core/Cargo.toml | | crates/harness-core/Cargo.toml::license.workspace | R-52.1 |
| M11-6 | M11 | Publication | open | | | | AC-R1, AC-R2 |
| M4b-1 | M4b | Subagent roles and checked answer | built | crates/harness-core/src/subagent.rs | inputs_are_cut_and_counted_at_the_limits, a_reference_to_an_unknown_input_is_dropped_and_counted, answers_outside_the_contract_are_refused, a_bare_line_number_refers_to_the_only_input | crates/harness-runtime/src/subagent.rs::run_for_config | R-12.2, R-12.3, R-13.1, R-13.2 |
| M4b-2 | M4b | Subagent inputs gated and redacted | built | crates/harness-runtime/src/subagent.rs | a_policy_refused_path_is_an_error_and_is_not_read, secrets_in_an_allowed_file_are_redacted_before_the_model_sees_them | crates/harness-runtime/src/subagent.rs::gather_inputs | R-13.2, R-27.2 |
| M4b-3 | M4b | Subagent model call and fail-honest answer | built | crates/harness-runtime/src/subagent.rs | a_cited_answer_reaches_the_parent_with_its_reference, an_unreachable_model_is_an_error_that_names_it | crates/harness-runtime/src/subagent.rs::answer | R-18.3 |
| M4b-4 | M4b | Subagent config and validation V37, V38 | built | crates/harness-core/src/config/schema.rs, crates/harness-core/src/config/validate.rs | v37_subagents_need_a_defined_slot, v38_subagent_limits_are_bounded | crates/harness-core/src/config/validate.rs::check_subagents | R-12.2 |
| M4b-5 | M4b | Subagent tool offered to the model | built | crates/harness-runtime/src/mcp/tools/subagent.rs | the_subagent_tool_is_offered_only_when_the_tier_is_on, a_bad_role_is_refused_by_the_tool_before_any_model_call | crates/harness-cli/src/commands/mcp.rs::SubagentTool | R-12.2 |
| M4b-6 | M4b | Subagent command for manual use | built | crates/harness-cli/src/commands/subagent.rs | the_command_refuses_when_the_tier_is_off, an_unreachable_model_is_reported_as_unavailable_and_nothing_is_answered | crates/harness-cli/src/main.rs::Command::Subagent | R-12.2 |
| M4b-7 | M4b | Subagent live check against the micro model | built | docs/status/milestones/m4b-subagents.md | | crates/harness-cli/src/commands/subagent.rs::run | R-12.3 |
| M4b-8 | M4b | Subagent quality measured over repeated runs | built | evals/subagent/cases.json, evals/subagent/results/2026-10-27-micro.json | | evals/subagent/run.py::main | R-12.3 |

## Open requirements without a row above

These are in the register and not built. Each has a reason, so none is silently missing.

| Requirement | Why it is open |
|---|---|
| R-18.2 triggers for the deep model | Routing to the deep model from loop rules is not built; the deep slot is only used by the evaluation. |
| R-22 research task mode | The schema and the experiment fields are not implemented. |
| R-29.1 deterministic routing between model slots | Not built; the slots are chosen by the user and the policy, not routed. |
| R-47.1 cost-aware routing | Routing by expected gain is not built (M10 and later in the register). |
